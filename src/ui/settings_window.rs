//! The settings window, laid out like the Windows 11 Settings app: one card per setting
//! with an icon, a title, a short description and its control on the right. Changes apply
//! right away, as in Windows. Text fields are standard edit controls (IME, clipboard and
//! screen readers work as usual); everything else is drawn into the shared canvas.

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use super::dialogs::{self, create_control, message_box, set_theme_title, text_of};
use super::gfx::{self, make_font, Canvas, DT_C, DT_L};
use super::theme::{self, Palette};
use super::theme_visuals::paint_motif;
use crate::app;
use crate::settings::{Hotkey, Settings, Theme, MAX_ITEMS_LIMIT, MAX_ITEM_MB_LIMIT};
use crate::system;
use crate::util::{self, wide};

thread_local! {
    static UI: RefCell<Option<SettingsUi>> = const { RefCell::new(None) };
}

const WIDTH_DIP: f32 = 700.0;
const HEIGHT_DIP: f32 = 780.0;
const WINDOW_STYLE: u32 = wm::WS_OVERLAPPEDWINDOW | wm::WS_CLIPCHILDREN;
const THEME_COLUMNS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Switch {
    History,
    Pause,
    WinV,
    Paste,
    SavePinned,
    KeepHistory,
    Group,
    Autostart,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Hotkey,
    MaxItems,
    MaxMb,
    Retention,
    Excluded,
}

impl Field {
    const ALL: [Field; 5] = [Field::Hotkey, Field::MaxItems, Field::MaxMb, Field::Retention, Field::Excluded];
    fn id(self) -> i32 {
        200 + self as i32
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Command {
    PhoneLink,
    WindowsSettings,
    Snippet,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Back,
    Switch(Switch),
    Field(Field),
    Theme(Theme),
    Button(Command),
}

enum Kind {
    Switch(Switch),
    Field(Field, f32),
    List(Field),
    Theme,
    Button(Command, &'static str),
}

struct Row {
    section: Option<&'static str>,
    glyph: &'static str,
    title: &'static str,
    kind: Kind,
}

fn rows() -> Vec<Row> {
    let row = |section, glyph, title, kind| Row { section, glyph, title, kind };
    vec![
        row(Some("Clipboard history"), "\u{E77F}", "Clipboard history", Kind::Switch(Switch::History)),
        row(None, "\u{E769}", "Pause recording", Kind::Switch(Switch::Pause)),
        row(None, "\u{E765}", "Open with Win+V", Kind::Switch(Switch::WinV)),
        row(None, "\u{E92E}", "Shortcut", Kind::Field(Field::Hotkey, 170.0)),
        row(None, "\u{E8C8}", "Paste after choosing an item", Kind::Switch(Switch::Paste)),
        row(Some("Storage"), "\u{E72E}", "Save pinned items", Kind::Switch(Switch::SavePinned)),
        row(None, "\u{E81C}", "Keep all items after a restart", Kind::Switch(Switch::KeepHistory)),
        row(None, "\u{E8F1}", "Group identical copies", Kind::Switch(Switch::Group)),
        row(None, "\u{E8FD}", "Items to keep", Kind::Field(Field::MaxItems, 96.0)),
        row(None, "\u{E7B8}", "Largest item", Kind::Field(Field::MaxMb, 96.0)),
        row(None, "\u{E916}", "Remove items after", Kind::Field(Field::Retention, 96.0)),
        row(Some("Privacy"), "\u{E72E}", "Never record from these apps", Kind::List(Field::Excluded)),
        row(Some("Appearance and startup"), "\u{E790}", "App theme", Kind::Theme),
        row(None, "\u{E7E8}", "Start when I sign in", Kind::Switch(Switch::Autostart)),
        row(
            Some("Windows and your phone"),
            "\u{E8EA}",
            "Phone Link",
            Kind::Button(Command::PhoneLink, "Open Phone Link"),
        ),
        row(None, "\u{E713}", "Windows clipboard settings", Kind::Button(Command::WindowsSettings, "Open")),
        row(None, "\u{E70F}", "Snippets", Kind::Button(Command::Snippet, "New snippet")),
        row(None, "\u{E74D}", "Clear clipboard data", Kind::Button(Command::Clear, "Clear")),
    ]
}

struct Fonts {
    page: HFONT,
    section: HFONT,
    title: HFONT,
    body: HFONT,
    caption: HFONT,
    icon: HFONT,
}

impl Fonts {
    fn new(scale: f32) -> Fonts {
        let px = |dip: f32| (dip * scale).round() as i32;
        let face = if gfx::font_exists("Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
        let display = if gfx::font_exists("Segoe UI Variable Display") { "Segoe UI Variable Display" } else { face };
        let icons = if gfx::font_exists("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        Fonts {
            page: make_font(display, px(28.0), 600),
            section: make_font(face, px(14.0), 600),
            title: make_font(face, px(14.0), 400),
            body: make_font(face, px(14.0), 400),
            caption: make_font(face, px(12.0), 400),
            icon: make_font(icons, px(20.0), 400),
        }
    }
    fn release(&self) {
        for f in [self.page, self.section, self.title, self.body, self.caption, self.icon] {
            unsafe { DeleteObject(f) };
        }
    }
}

/// A placed card, in content coordinates (before scrolling).
struct Card {
    row: usize,
    y: i32,
    h: i32,
    section_y: Option<i32>,
    control: (i32, i32, i32, i32),
    description: String,
    description_h: i32,
    error: Option<String>,
}

struct SettingsUi {
    window: HWND,
    canvas: Canvas,
    icon: wm::HICON,
    fonts: Fonts,
    scale: f32,
    palette: Palette,
    theme_previews: [Palette; Theme::ALL.len()],
    preview_art: Option<Canvas>,
    from_panel: bool,
    field_brush: HBRUSH,
    edits: Vec<(Field, HWND)>,
    rows: Vec<Row>,
    cards: Vec<Card>,
    content_h: i32,
    scroll: i32,
    hover: Option<Control>,
    focus: Option<Control>,
    errors: Vec<(Field, String)>,
    settings: Settings,
    win_v: bool,
    autostart: bool,
    notice: Option<String>,
    policy_blocked: bool,
    windows_clipboard: (Option<bool>, Option<bool>),
}

/// Work that must run after the state borrow ends (message boxes, other windows).
enum Deferred {
    Back,
    Toggle(Switch),
    Theme(Theme),
    Command(Command),
    Commit(Field),
}

fn with_ui<R>(f: impl FnOnce(&mut SettingsUi) -> R) -> Option<R> {
    UI.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn field_text(settings: &Settings, field: Field) -> String {
    match field {
        Field::Hotkey => settings.hotkey.label(),
        Field::MaxItems => settings.max_items.to_string(),
        Field::MaxMb => settings.max_item_mb.to_string(),
        Field::Retention => settings.retention_days.to_string(),
        Field::Excluded => settings.excluded_apps.join("\r\n"),
    }
}

fn window_size(dpi: u32) -> (i32, i32) {
    let scale = dpi as f32 / 96.0;
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: (WIDTH_DIP * scale).round() as i32,
        bottom: (HEIGHT_DIP * scale).round() as i32,
    };
    unsafe { AdjustWindowRectExForDpi(&mut frame, WINDOW_STYLE, 0, 0, dpi) };
    (frame.right - frame.left, frame.bottom - frame.top)
}

fn windows_clipboard_status() -> (Option<bool>, Option<bool>) {
    (system::windows_history_enabled(), system::windows_cloud_sync_enabled())
}

pub fn open_settings(from_panel: bool) {
    if let Some(window) = with_ui(|ui| {
        let mode_changed = ui.from_panel != from_panel;
        ui.from_panel = from_panel;
        if unsafe { wm::IsWindowVisible(ui.window) } == 0 {
            ui.reset_for_open(mode_changed);
        } else {
            if mode_changed && ui.focus == Some(Control::Back) {
                ui.focus = None;
            }
            ui.refresh_from_app(mode_changed);
        }
        ui.window
    }) {
        unsafe {
            let show = if wm::IsIconic(window) != 0 { wm::SW_RESTORE } else { wm::SW_SHOW };
            wm::ShowWindow(window, show);
            wm::SetForegroundWindow(window);
        }
        return;
    }
    let Some((settings, notice)) = app::with(|a| (a.settings.clone(), a.notice.clone())) else { return };
    let class = format!("BetterClipboard.Settings{}", util::profile());
    dialogs::register(&class, Some(settings_proc));
    let initial_dpi = unsafe { GetDpiForSystem() }.max(96);
    let (w, h) = window_size(initial_dpi);
    let window = unsafe {
        wm::CreateWindowExW(
            0,
            wide(&class).as_ptr(),
            wide("BetterClipboard Settings").as_ptr(),
            WINDOW_STYLE,
            wm::CW_USEDEFAULT,
            wm::CW_USEDEFAULT,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return;
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let scale = dpi as f32 / 96.0;
    let theme_previews = theme::preview_palettes();
    let palette = theme_previews[theme_index(settings.theme)];
    set_theme_title(window, palette);

    // Standard edit controls for the text fields.
    const ES_AUTOHSCROLL: u32 = 0x80;
    const ES_MULTILINE: u32 = 4;
    const ES_AUTOVSCROLL: u32 = 0x40;
    const ES_WANTRETURN: u32 = 0x1000;
    let mut edits = Vec::new();
    for field in Field::ALL {
        let style = if field == Field::Excluded {
            ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | wm::WS_VSCROLL
        } else {
            ES_AUTOHSCROLL
        };
        let edit = create_control(window, "EDIT", &field_text(&settings, field), style | wm::WS_TABSTOP, 0, field.id());
        edits.push((field, edit));
    }

    let mut ui = SettingsUi {
        window,
        canvas: Canvas::new(),
        icon: crate::tray::app_icon((40.0 * scale).round() as i32),
        fonts: Fonts::new(scale),
        scale,
        palette,
        theme_previews,
        preview_art: None,
        from_panel,
        field_brush: unsafe { CreateSolidBrush(gfx::cr(palette.field)) },
        edits,
        rows: rows(),
        cards: Vec::new(),
        content_h: 0,
        scroll: 0,
        hover: None,
        focus: None,
        errors: Vec::new(),
        win_v: settings.hotkey == Hotkey::WIN_V && system::windows_win_v_disabled(),
        autostart: system::autostart_enabled(),
        settings,
        notice,
        policy_blocked: system::history_blocked_by_policy(),
        windows_clipboard: windows_clipboard_status(),
    };
    ui.apply_fonts();
    if dpi != initial_dpi {
        let (w, h) = window_size(dpi);
        unsafe { wm::SetWindowPos(window, std::ptr::null_mut(), 0, 0, w, h, wm::SWP_NOMOVE | wm::SWP_NOZORDER) };
    }
    ui.layout();
    UI.with(|cell| *cell.borrow_mut() = Some(ui));
    app::with(|a| a.settings_window = window);
    unsafe {
        wm::ShowWindow(window, wm::SW_SHOWNORMAL);
        wm::SetForegroundWindow(window);
    }
}

impl SettingsUi {
    fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }

    fn client(&self) -> (i32, i32) {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { wm::GetClientRect(self.window, &mut r) };
        (r.right - r.left, r.bottom - r.top)
    }

    fn apply_fonts(&self) {
        for (_, edit) in &self.edits {
            unsafe { wm::SendMessageW(*edit, wm::WM_SETFONT, self.fonts.body as usize, 1) };
        }
    }

    fn reset_for_open(&mut self, mode_changed: bool) {
        self.focus = None;
        self.hover = None;
        let needs_layout = mode_changed || !self.errors.is_empty();
        let was_scrolled = self.scroll != 0;
        self.scroll = 0;
        self.errors.clear();
        if !self.refresh_from_app(needs_layout) && was_scrolled {
            self.place_edits();
        }
        for (field, edit) in &self.edits {
            let expected = field_text(&self.settings, *field);
            if text_of(self.window, field.id()) != expected {
                unsafe { wm::SetWindowTextW(*edit, wide(&expected).as_ptr()) };
            }
        }
        unsafe { InvalidateRect(self.window, std::ptr::null(), 0) };
    }

    fn refresh_from_app(&mut self, force_layout: bool) -> bool {
        let Some((settings, notice)) = app::with(|a| (a.settings.clone(), a.notice.clone())) else {
            return false;
        };
        let win_v = settings.hotkey == Hotkey::WIN_V && system::windows_win_v_disabled();
        let autostart = system::autostart_enabled();
        let policy_blocked = system::history_blocked_by_policy();
        let windows_clipboard = windows_clipboard_status();
        if !force_layout
            && self.settings == settings
            && self.notice == notice
            && self.win_v == win_v
            && self.autostart == autostart
            && self.policy_blocked == policy_blocked
            && self.windows_clipboard == windows_clipboard
        {
            return false;
        }
        let theme_changed = self.settings.theme != settings.theme;
        self.settings = settings;
        self.notice = notice;
        self.win_v = win_v;
        self.autostart = autostart;
        self.policy_blocked = policy_blocked;
        self.windows_clipboard = windows_clipboard;
        if theme_changed {
            self.set_palette();
        }
        for (field, edit) in &self.edits {
            // A field with an error keeps what the user typed until it is fixed.
            let has_error = self.errors.iter().any(|(f, _)| f == field);
            if *field != Field::Excluded && !has_error && unsafe { GetFocus() } != *edit {
                unsafe { wm::SetWindowTextW(*edit, wide(&field_text(&self.settings, *field)).as_ptr()) };
            }
        }
        self.layout();
        true
    }

    fn set_palette(&mut self) {
        self.palette = self.theme_previews[theme_index(self.settings.theme)];
        set_theme_title(self.window, self.palette);
        unsafe {
            DeleteObject(self.field_brush);
            self.field_brush = CreateSolidBrush(gfx::cr(self.palette.field));
            for (_, edit) in &self.edits {
                InvalidateRect(*edit, std::ptr::null(), 1);
            }
            InvalidateRect(self.window, std::ptr::null(), 0);
        }
    }

    fn description(&self, row: &Row) -> String {
        match &row.kind {
            Kind::Switch(Switch::History) if self.policy_blocked => "Turned off by your organization.".into(),
            Kind::Switch(Switch::History) => format!(
                "Save multiple items to the clipboard to use later. Press {} to view your history.",
                self.settings.hotkey.label()
            ),
            Kind::Switch(Switch::Pause) => "Stop adding new items for now. Everything you have stays.".into(),
            Kind::Switch(Switch::WinV) => {
                "Replace the Windows clipboard panel. Explorer restarts once so it applies without signing out.".into()
            }
            Kind::Field(Field::Hotkey, _) if self.win_v => "Win+V opens BetterClipboard.".into(),
            Kind::Field(Field::Hotkey, _) => {
                "Opens the clipboard panel. For example Win+Alt+V or Ctrl+Shift+Space.".into()
            }
            Kind::Switch(Switch::Paste) => "Off: choosing an item only copies it.".into(),
            Kind::Switch(Switch::SavePinned) => {
                "Encrypted for your Windows account. Off: nothing is ever written to disk.".into()
            }
            Kind::Switch(Switch::KeepHistory) => "Windows clears unpinned items when the PC restarts.".into(),
            Kind::Switch(Switch::Group) => "Copying the same thing again moves it to the top.".into(),
            Kind::Field(Field::MaxItems, _) => "Unpinned items. Windows keeps 25.".into(),
            Kind::Field(Field::MaxMb, _) => "In MB. Windows uses 4; screenshots may use up to 32.".into(),
            Kind::Field(Field::Retention, _) => "In days. 0 keeps items until the limit is reached.".into(),
            Kind::List(_) => {
                "One program file per line. Password managers that mark copies as private are skipped anyway.".into()
            }
            Kind::Theme => "Choose how the panel and this window look.".into(),
            Kind::Switch(Switch::Autostart) => "Keeps the shortcut ready after you sign in.".into(),
            Kind::Button(Command::PhoneLink, _) => {
                "Turn on \"Cross-device copy and paste\" in Phone Link. What you copy on your Android phone then shows up here with a phone icon.".into()
            }
            Kind::Button(Command::WindowsSettings, _) => {
                let state = |enabled| match enabled {
                    Some(true) => "on",
                    Some(false) => "off",
                    None => "not set",
                };
                let windows = state(self.windows_clipboard.0);
                let sync = state(self.windows_clipboard.1);
                format!("Windows clipboard history is {windows}, sync across your devices is {sync}.")
            }
            Kind::Button(Command::Snippet, _) => "Type text to keep as a pinned item.".into(),
            Kind::Button(Command::Clear, _) => {
                "Removes everything except pinned items and empties the clipboard.".into()
            }
            _ => String::new(),
        }
    }

    fn control_size(&self, kind: &Kind) -> (i32, i32) {
        match kind {
            Kind::Switch(_) => (self.px(40.0 + 12.0 + 28.0), self.px(20.0)),
            Kind::Field(_, w) => (self.px(*w), self.px(32.0)),
            Kind::List(_) => (0, 0),
            Kind::Theme => (0, self.px(192.0)),
            Kind::Button(_, label) => (self.canvas.text_width(self.fonts.body, label) + self.px(32.0), self.px(32.0)),
        }
    }

    fn text_height(&self, font: HFONT, text: &str, width: i32) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut r = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
        unsafe {
            SelectObject(self.canvas.hdc, font);
            DrawTextW(
                self.canvas.hdc,
                units.as_ptr(),
                units.len() as i32,
                &mut r,
                DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX,
            );
        }
        r.bottom - r.top
    }

    fn content_x(&self) -> (i32, i32) {
        let (w, _) = self.client();
        let margin = self.px(32.0);
        let extra = if self.from_panel { self.px(16.0) } else { 0 };
        let width = (w - 2 * margin - extra).min(self.px(1000.0));
        (((w + extra - width) / 2).max((margin + extra).min(w / 2)), width.max(self.px(200.0)))
    }

    fn back_rect(&self) -> (i32, i32, i32, i32) {
        let (x, _) = self.content_x();
        (x - self.px(42.0), self.px(28.0), self.px(34.0), self.px(34.0))
    }

    fn header_h(&self) -> i32 {
        let notice = if self.notice.is_some() { self.px(64.0) } else { 0 };
        self.px(108.0) + notice
    }

    fn layout(&mut self) {
        let (w, h) = self.client();
        self.canvas.ensure(w, h);
        let (x, width) = self.content_x();
        let mut y = self.header_h();
        let mut cards = Vec::new();
        let gap = self.px(4.0);
        for (index, row) in self.rows.iter().enumerate() {
            let mut section_y = None;
            if row.section.is_some() {
                y += self.px(20.0);
                section_y = Some(y);
                y += self.px(32.0);
            }
            let (cw, ch) = self.control_size(&row.kind);
            let text_w = width - self.px(68.0) - cw - self.px(36.0);
            let description = self.description(row);
            let error = match &row.kind {
                Kind::Field(f, _) | Kind::List(f) => {
                    self.errors.iter().find(|(e, _)| e == f).map(|(_, message)| message.clone())
                }
                _ => None,
            };
            let stacked = matches!(row.kind, Kind::List(_) | Kind::Theme);
            let text_w = if stacked { width - self.px(84.0) } else { text_w };
            let desc_h = self.text_height(self.fonts.caption, &description, text_w);
            let error_h = error.as_ref().map_or(0, |e| self.text_height(self.fonts.caption, e, text_w) + self.px(4.0));
            let text_h = self.px(20.0) + desc_h + error_h;
            let mut card_h = (text_h + self.px(28.0)).max(self.px(68.0));
            let control_y;
            if stacked {
                control_y = y + self.px(14.0) + text_h + self.px(10.0);
                let control_h = if matches!(row.kind, Kind::List(_)) { self.px(100.0) } else { ch };
                card_h = control_y - y + control_h + self.px(16.0);
            } else {
                control_y = y + (card_h - ch) / 2;
            }
            let control = match row.kind {
                Kind::List(_) => (x + self.px(68.0), control_y, width - self.px(84.0), self.px(100.0)),
                Kind::Theme => (x + self.px(16.0), control_y, width - self.px(32.0), ch),
                _ => (x + width - self.px(20.0) - cw, control_y, cw, ch),
            };
            cards.push(Card {
                row: index,
                y,
                h: card_h,
                section_y,
                control,
                description,
                description_h: desc_h,
                error,
            });
            y += card_h + gap;
        }
        y += self.px(16.0);
        self.content_h = y + self.px(40.0);
        self.cards = cards;
        let max = (self.content_h - h).max(0);
        self.scroll = self.scroll.clamp(0, max);
        self.place_edits();
        unsafe { InvalidateRect(self.window, std::ptr::null(), 0) };
    }

    /// Edit rectangles in window coordinates (inside the drawn field frame).
    fn place_edits(&self) {
        for card in &self.cards {
            let field = match self.rows[card.row].kind {
                Kind::Field(f, _) | Kind::List(f) => f,
                _ => continue,
            };
            let Some((_, edit)) = self.edits.iter().find(|(f, _)| *f == field) else { continue };
            let (x, y, w, h) = card.control;
            let y = y - self.scroll;
            let inset = self.px(10.0);
            let (ex, ey, ew, eh) = if field == Field::Excluded {
                (x + inset, y + self.px(6.0), w - 2 * inset, h - self.px(12.0))
            } else {
                let line = self.px(20.0);
                (x + inset, y + (h - line) / 2, w - 2 * inset, line)
            };
            unsafe {
                wm::SetWindowPos(*edit, std::ptr::null_mut(), ex, ey, ew, eh, wm::SWP_NOZORDER | wm::SWP_NOACTIVATE);
            }
        }
    }

    fn control_at(&self, x: i32, y: i32) -> Option<Control> {
        let (back_x, back_y, back_w, back_h) = self.back_rect();
        if self.from_panel && x >= back_x && x < back_x + back_w && y >= back_y && y < back_y + back_h {
            return Some(Control::Back);
        }
        let y = y + self.scroll;
        let (card_x, width) = self.content_x();
        for card in &self.cards {
            let (cx, cy, cw, ch) = card.control;
            let inside = x >= cx && x < cx + cw && y >= cy && y < cy + ch;
            match self.rows[card.row].kind {
                Kind::Switch(s) => {
                    // The whole card toggles, like in Windows Settings.
                    if y >= card.y && y < card.y + card.h && x >= card_x && x < card_x + width {
                        return Some(Control::Switch(s));
                    }
                }
                Kind::Field(f, _) | Kind::List(f) if inside => return Some(Control::Field(f)),
                Kind::Theme if inside => {
                    return Theme::ALL.into_iter().enumerate().find_map(|(index, theme)| {
                        let (tx, ty, tw, th) = theme_cell(card.control, index, self.px(8.0));
                        (x >= tx && x < tx + tw && y >= ty && y < ty + th).then_some(Control::Theme(theme))
                    });
                }
                Kind::Button(c, _) if inside => return Some(Control::Button(c)),
                _ => {}
            }
        }
        None
    }

    /// Controls in Tab order.
    fn focusables(&self) -> Vec<Control> {
        self.from_panel
            .then_some(Control::Back)
            .into_iter()
            .chain(self.rows.iter().map(|row| match row.kind {
                Kind::Switch(s) => Control::Switch(s),
                Kind::Field(f, _) | Kind::List(f) => Control::Field(f),
                Kind::Theme => Control::Theme(self.settings.theme),
                Kind::Button(c, _) => Control::Button(c),
            }))
            .collect()
    }

    fn move_focus(&mut self, backwards: bool) {
        let list = self.focusables();
        let current = self.focus.and_then(|f| {
            list.iter().position(|c| match (c, f) {
                (Control::Theme(_), Control::Theme(_)) => true,
                _ => *c == f,
            })
        });
        let next = match (current, backwards) {
            (None, false) => 0,
            (None, true) => list.len() - 1,
            (Some(i), false) => (i + 1) % list.len(),
            (Some(i), true) => (i + list.len() - 1) % list.len(),
        };
        self.set_focus(list[next]);
    }

    fn set_focus(&mut self, control: Control) {
        self.focus = Some(control);
        match control {
            Control::Field(field) => {
                if let Some((_, edit)) = self.edits.iter().find(|(f, _)| *f == field) {
                    unsafe {
                        SetFocus(*edit);
                        // EM_SETSEL: select everything.
                        wm::SendMessageW(*edit, 0x00B1, 0, -1isize as LPARAM);
                    }
                }
            }
            _ => unsafe {
                SetFocus(self.window);
            },
        }
        self.scroll_into_view(control);
        unsafe { InvalidateRect(self.window, std::ptr::null(), 0) };
    }

    fn scroll_into_view(&mut self, control: Control) {
        let (_, h) = self.client();
        let Some(card) = self.cards.iter().find(|card| match (&self.rows[card.row].kind, control) {
            (Kind::Switch(a), Control::Switch(b)) => *a == b,
            (Kind::Field(a, _) | Kind::List(a), Control::Field(b)) => *a == b,
            (Kind::Theme, Control::Theme(_)) => true,
            (Kind::Button(a, _), Control::Button(b)) => *a == b,
            _ => false,
        }) else {
            return;
        };
        let (top, bottom) = (card.section_y.unwrap_or(card.y) - self.px(8.0), card.y + card.h + self.px(8.0));
        if top < self.scroll {
            self.scroll = top.max(0);
        } else if bottom > self.scroll + h {
            self.scroll = bottom - h;
        }
        self.place_edits();
    }

    fn scroll_by(&mut self, delta: i32) {
        let (_, h) = self.client();
        self.scroll = (self.scroll + delta).clamp(0, (self.content_h - h).max(0));
        self.place_edits();
        unsafe { InvalidateRect(self.window, std::ptr::null(), 0) };
    }

    fn cache_theme_art(&mut self, height: i32) {
        let Some(card) = self.cards.iter().find(|card| matches!(self.rows[card.row].kind, Kind::Theme)) else {
            return;
        };
        let (_, _, width, grid_h) = card.control;
        let y = card.y - self.scroll;
        if y >= height || y + card.h <= 0 {
            return;
        }
        if self.preview_art.as_ref().is_some_and(|c| c.w == width && c.h == grid_h) {
            return;
        }
        let mut art = self.preview_art.take().unwrap_or_default();
        art.ensure(width, grid_h);
        for (index, (theme, preview)) in Theme::ALL.into_iter().zip(self.theme_previews).enumerate() {
            let cell = theme_cell((0, 0, width, grid_h), index, self.px(8.0));
            paint_theme_sample(&mut art, theme_sample(cell, self.scale), self.scale, theme, preview);
        }
        self.preview_art = Some(art);
    }

    fn paint(&mut self, dc: HDC) {
        let (w, h) = self.client();
        self.canvas.ensure(w, h);
        let p = self.palette;
        let s = self.scale;
        let px = |dip: f32| (dip * s).round() as i32;
        let pxf = |dip: f32| dip * s;
        let (x, width) = self.content_x();
        let scroll = self.scroll;
        let back_rect = self.from_panel.then(|| self.back_rect());
        self.cache_theme_art(h);
        let (card_fill, card_hover) = if self.settings.theme.is_custom() || p.contrast {
            (p.card, p.card_hover)
        } else if p.dark {
            (0x2B2B2B, 0x323232)
        } else {
            (0xFBFBFB, 0xF6F6F6)
        };
        let switches = [
            Switch::History,
            Switch::Pause,
            Switch::WinV,
            Switch::Paste,
            Switch::SavePinned,
            Switch::KeepHistory,
            Switch::Group,
            Switch::Autostart,
        ]
        .map(|sw| (sw, self.switch_value(sw)));
        let c = &mut self.canvas;
        c.fill(0, 0, w, h, p.background);

        // Page header: icon, name, version.
        let top = px(28.0) - scroll;
        if p.motifs && top + px(56.0) > 0 {
            c.fill(0, -scroll, w, px(92.0), p.header);
            paint_motif(c.hdc, (x + px(292.0), top, width - px(292.0), px(40.0)), self.settings.theme, p);
        }
        if top + px(48.0) > 0 {
            if !self.icon.is_null() {
                unsafe {
                    wm::DrawIconEx(
                        c.hdc,
                        x,
                        top,
                        self.icon,
                        px(40.0),
                        px(40.0),
                        0,
                        std::ptr::null_mut(),
                        wm::DI_NORMAL,
                    );
                }
            }
            c.text(self.fonts.page, "BetterClipboard", (x + px(56.0), top - px(4.0), width, px(36.0)), p.text, DT_L);
            let version = format!("Version {} \u{00B7} clipboard history for Windows", crate::install::VERSION);
            c.text(self.fonts.caption, &version, (x + px(56.0), top + px(30.0), width, px(18.0)), p.secondary, DT_L);
        }

        // Info bar for problems with the shortcut and similar notices.
        if let Some(notice) = &self.notice {
            let ny = px(96.0) - scroll;
            let warn = if self.settings.theme.is_custom() || p.contrast {
                p.banner
            } else if p.dark {
                0x433519
            } else {
                0xFFF4CE
            };
            c.round_rect(x as f32, ny as f32, width as f32, pxf(52.0), pxf(6.0), warn, 1.0);
            c.text(self.fonts.icon, "\u{E7BA}", (x + px(16.0), ny, px(24.0), px(52.0)), p.text, DT_C);
            c.text(self.fonts.body, notice, (x + px(52.0), ny, width - px(68.0), px(52.0)), p.text, DT_L);
        }

        for card in &self.cards {
            let row = &self.rows[card.row];
            if let (Some(section), Some(sy)) = (row.section, card.section_y) {
                let sy = sy - scroll;
                if sy < h && sy + px(28.0) > 0 {
                    c.text(self.fonts.section, section, (x + px(2.0), sy, width, px(28.0)), p.text, DT_L);
                }
            }
            let y = card.y - scroll;
            if y > h || y + card.h < 0 {
                continue;
            }
            let control = match row.kind {
                Kind::Switch(sw) => Some(Control::Switch(sw)),
                Kind::Button(cm, _) => Some(Control::Button(cm)),
                _ => None,
            };
            let hot = control.is_some() && matches!(row.kind, Kind::Switch(_)) && self.hover == control;
            c.round_rect(
                x as f32,
                y as f32,
                width as f32,
                card.h as f32,
                pxf(6.0),
                if hot { card_hover } else { card_fill },
                1.0,
            );
            c.round_rect_outline(x as f32, y as f32, width as f32, card.h as f32, pxf(6.0), 1.0, p.card_border, 1.0);

            let disabled = matches!(row.kind, Kind::Switch(Switch::History)) && self.policy_blocked
                || matches!(row.kind, Kind::Switch(Switch::Pause)) && !self.settings.history_enabled;
            let text_color = if disabled { p.tertiary } else { p.text };
            let stacked = matches!(row.kind, Kind::List(_) | Kind::Theme);
            c.text(
                self.fonts.icon,
                row.glyph,
                (x + px(20.0), y, px(28.0), if stacked { px(68.0) } else { card.h }),
                text_color,
                DT_C,
            );
            let (cx, cy, cw, ch) = card.control;
            let cy = cy - scroll;
            let text_right = if stacked { x + width - px(16.0) } else { cx - px(16.0) };
            let text_w = text_right - (x + px(68.0));
            let desc_h = card.description_h;
            let error_h = card.error.as_ref().map_or(0, |_| px(20.0));
            let block_h = px(20.0) + desc_h + error_h;
            let ty = if stacked { y + px(14.0) } else { y + (card.h - block_h) / 2 };
            c.text(self.fonts.title, row.title, (x + px(68.0), ty, text_w, px(20.0)), text_color, DT_L);
            c.text(
                self.fonts.caption,
                &card.description,
                (x + px(68.0), ty + px(20.0), text_w, desc_h),
                p.secondary,
                DT_LEFT | DT_TOP | DT_WORDBREAK,
            );
            if let Some(error) = &card.error {
                let error_color = if p.contrast {
                    p.text
                } else if p.dark {
                    0xFF99A4
                } else {
                    0xC42B1C
                };
                c.text(
                    self.fonts.caption,
                    error,
                    (x + px(68.0), ty + px(20.0) + desc_h + px(2.0), text_w, px(18.0)),
                    error_color,
                    DT_L,
                );
            }

            let focused = |control: Control| self.focus == Some(control);
            match row.kind {
                Kind::Switch(sw) => {
                    let on = switches.iter().any(|(k, v)| *k == sw && *v);
                    let label = if on { "On" } else { "Off" };
                    c.text(self.fonts.body, label, (cx, cy - px(2.0), px(28.0), ch + px(4.0)), text_color, DT_L);
                    let (sx, sy, sw_w, sh) = (cx + px(40.0), cy, px(40.0), px(20.0));
                    if on && !disabled {
                        c.round_rect(sx as f32, sy as f32, sw_w as f32, sh as f32, sh as f32 / 2.0, p.accent, 1.0);
                        let knob = px(12.0);
                        c.round_rect(
                            (sx + sw_w - px(4.0) - knob) as f32,
                            (sy + (sh - knob) / 2) as f32,
                            knob as f32,
                            knob as f32,
                            knob as f32 / 2.0,
                            p.on_accent,
                            1.0,
                        );
                    } else {
                        let edge = if disabled { p.tertiary } else { p.secondary };
                        c.round_rect_outline(
                            sx as f32,
                            sy as f32,
                            sw_w as f32,
                            sh as f32,
                            sh as f32 / 2.0,
                            pxf(1.0),
                            edge,
                            1.0,
                        );
                        let knob = px(12.0);
                        let kx = if on { sx + sw_w - px(4.0) - knob } else { sx + px(4.0) };
                        c.round_rect(
                            kx as f32,
                            (sy + (sh - knob) / 2) as f32,
                            knob as f32,
                            knob as f32,
                            knob as f32 / 2.0,
                            edge,
                            1.0,
                        );
                    }
                    if focused(Control::Switch(sw)) {
                        c.round_rect_outline(
                            x as f32 + pxf(2.0),
                            y as f32 + pxf(2.0),
                            width as f32 - pxf(4.0),
                            card.h as f32 - pxf(4.0),
                            pxf(5.0),
                            pxf(2.0),
                            p.focus,
                            1.0,
                        );
                    }
                }
                Kind::Field(f, _) | Kind::List(f) => {
                    let active = focused(Control::Field(f));
                    c.round_rect(cx as f32, cy as f32, cw as f32, ch as f32, pxf(4.0), p.field, 1.0);
                    c.round_rect_outline(
                        cx as f32,
                        cy as f32,
                        cw as f32,
                        ch as f32,
                        pxf(4.0),
                        1.0,
                        p.field_border,
                        1.0,
                    );
                    let underline = if active { p.accent } else { p.tertiary };
                    let thickness = if active { px(2.0) } else { px(1.0) };
                    c.fill(cx + px(3.0), cy + ch - thickness, cw - px(6.0), thickness, underline);
                }
                Kind::Theme => {
                    for (i, (theme, preview)) in Theme::ALL.into_iter().zip(self.theme_previews).enumerate() {
                        let cell = theme_cell((cx, cy, cw, ch), i, px(8.0));
                        let (tx, ty, tw, th) = cell;
                        if ty >= h || ty + th <= 0 {
                            continue;
                        }
                        let selected = theme == self.settings.theme;
                        let focused = focused(Control::Theme(theme));
                        let hot = self.hover == Some(Control::Theme(theme));
                        let frame = if focused {
                            p.focus
                        } else if selected || hot {
                            p.accent
                        } else {
                            p.field_border
                        };
                        let stroke = if selected || focused { pxf(2.0) } else { pxf(1.0) };
                        c.round_rect(tx as f32, ty as f32, tw as f32, th as f32, pxf(5.0), preview.card, 1.0);
                        c.round_rect_outline(tx as f32, ty as f32, tw as f32, th as f32, pxf(5.0), stroke, frame, 1.0);
                        let sample = theme_sample(cell, s);
                        let (ax, ay, aw, ah) = sample;
                        let copied = self.preview_art.as_ref().is_some_and(|art| unsafe {
                            BitBlt(c.hdc, ax, ay, aw, ah, art.hdc, ax - cx, ay - cy, SRCCOPY) != 0
                        });
                        if !copied {
                            paint_theme_sample(c, sample, s, theme, preview);
                        }
                        c.text(
                            self.fonts.body,
                            theme.label(),
                            (tx + px(9.0), ty + th - px(23.0), tw - px(32.0), px(20.0)),
                            preview.text,
                            DT_L,
                        );
                        if selected {
                            c.text(
                                self.fonts.icon,
                                "\u{E73E}",
                                (tx + tw - px(26.0), ty + th - px(23.0), px(20.0), px(20.0)),
                                preview.text,
                                DT_C,
                            );
                        }
                    }
                }
                Kind::Button(cm, label) => {
                    let hot = self.hover == Some(Control::Button(cm));
                    let danger = cm == Command::Clear;
                    let fill = if hot { p.control_hover } else { p.card };
                    c.round_rect(cx as f32, cy as f32, cw as f32, ch as f32, pxf(4.0), fill, 1.0);
                    c.round_rect_outline(
                        cx as f32,
                        cy as f32,
                        cw as f32,
                        ch as f32,
                        pxf(4.0),
                        1.0,
                        p.field_border,
                        1.0,
                    );
                    let color = if danger && !p.contrast {
                        if p.dark {
                            0xFF99A4
                        } else {
                            0xC42B1C
                        }
                    } else {
                        p.text
                    };
                    c.text(self.fonts.body, label, (cx, cy, cw, ch), color, DT_C);
                    if focused(Control::Button(cm)) {
                        c.round_rect_outline(
                            cx as f32 - pxf(3.0),
                            cy as f32 - pxf(3.0),
                            cw as f32 + pxf(6.0),
                            ch as f32 + pxf(6.0),
                            pxf(6.0),
                            pxf(2.0),
                            p.focus,
                            1.0,
                        );
                    }
                }
            }
        }

        let footer_y = self.content_h - px(40.0) - scroll;
        if footer_y < h && footer_y + px(20.0) > 0 {
            let stats = app::with(|a| a.stats).unwrap_or_default();
            let line = format!(
                "This session: {} recorded, {} skipped as private, {} from excluded apps, {} too large.",
                stats.captured, stats.skipped_private, stats.skipped_excluded, stats.skipped_large
            );
            c.text(self.fonts.caption, &line, (x + px(2.0), footer_y, width, px(20.0)), p.tertiary, DT_L);
        }

        if let Some((bx, by, bw, bh)) = back_rect {
            let (fx, fy, fw, fh) = (bx as f32, by as f32, bw as f32, bh as f32);
            let fill = if self.hover == Some(Control::Back) { p.control_hover } else { p.card };
            c.round_rect(fx, fy, fw, fh, pxf(5.0), fill, 1.0);
            let (stroke, frame) =
                if self.focus == Some(Control::Back) { (pxf(2.0), p.focus) } else { (1.0, p.card_border) };
            c.round_rect_outline(fx, fy, fw, fh, pxf(5.0), stroke, frame, 1.0);
            c.text(self.fonts.icon, "\u{E72B}", (bx, by, bw, bh), p.text, DT_C);
        }

        // Scrollbar.
        if self.content_h > h {
            let thumb = ((h as f32 / self.content_h as f32) * h as f32).max(px(32.0) as f32) as i32;
            let max = (self.content_h - h).max(1);
            let ty = ((scroll as f32 / max as f32) * (h - thumb) as f32) as i32;
            c.round_rect((w - px(7.0)) as f32, ty as f32, pxf(3.0), thumb as f32, pxf(1.5), p.tertiary, 0.9);
        }
        c.present(dc, 0, 0, w, h);
    }

    fn switch_value(&self, switch: Switch) -> bool {
        let s = &self.settings;
        match switch {
            Switch::History => s.history_enabled && !self.policy_blocked,
            Switch::Pause => s.paused,
            Switch::WinV => self.win_v,
            Switch::Paste => s.paste_on_select,
            Switch::SavePinned => s.save_pinned,
            Switch::KeepHistory => s.keep_history,
            Switch::Group => s.group_duplicates,
            Switch::Autostart => self.autostart,
        }
    }
}

fn theme_index(theme: Theme) -> usize {
    Theme::ALL.iter().position(|&option| option == theme).expect("theme must have a preview")
}

fn theme_cell(grid: (i32, i32, i32, i32), index: usize, gap: i32) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = grid;
    let cols = THEME_COLUMNS as i32;
    let rows = Theme::ALL.len().div_ceil(THEME_COLUMNS) as i32;
    let col = index as i32 % cols;
    let row = index as i32 / cols;
    let offset = |length, position, count| (length + gap) * position / count;
    let left = offset(w, col, cols);
    let top = offset(h, row, rows);
    (x + left, y + top, offset(w, col + 1, cols) - left - gap, offset(h, row + 1, rows) - top - gap)
}

fn theme_sample((x, y, w, h): (i32, i32, i32, i32), scale: f32) -> (i32, i32, i32, i32) {
    let px = |dip: f32| (dip * scale).round() as i32;
    (x + px(5.0), y + px(5.0), w - px(10.0), h - px(29.0))
}

fn paint_theme_sample(canvas: &mut Canvas, (x, y, w, h): (i32, i32, i32, i32), scale: f32, theme: Theme, p: Palette) {
    let px = |dip: f32| (dip * scale).round() as i32;
    canvas.fill(x, y, w, h, p.header);
    canvas.fill(x + px(6.0), y + px(6.0), px(30.0), h - px(12.0), p.field);
    canvas.fill(x + px(9.0), y + px(9.0), px(13.0), px(2.0), p.secondary);
    canvas.fill(x + px(9.0), y + px(14.0), px(21.0), px(3.0), p.accent);
    paint_motif(canvas.hdc, (x + px(38.0), y, w - px(38.0), h), theme, p);
}

fn adjacent_theme(theme: Theme, key: u16) -> Theme {
    let index = theme_index(theme);
    let col = index % THEME_COLUMNS;
    let next = match key {
        0x25 if col > 0 => index - 1,                            // Left
        0x26 if index >= THEME_COLUMNS => index - THEME_COLUMNS, // Up
        0x27 if col + 1 < THEME_COLUMNS => index + 1,            // Right
        0x28 => index + THEME_COLUMNS,                           // Down
        _ => index,
    };
    Theme::ALL.get(next).copied().unwrap_or(theme)
}

/// Changes one setting through the app and redraws.
fn change(mutate: impl FnOnce(&mut Settings)) {
    app::with(|a| {
        let mut next = a.settings.clone();
        mutate(&mut next);
        a.apply_settings(next);
    });
    with_ui(|ui| ui.refresh_from_app(true));
}

fn owner() -> HWND {
    with_ui(|ui| ui.window).unwrap_or(std::ptr::null_mut())
}

fn run(deferred: Deferred) {
    match deferred {
        Deferred::Back => {
            commit_focused_field();
            app::with(app::App::resume_popup);
            close();
        }
        Deferred::Toggle(switch) => toggle(switch),
        Deferred::Theme(theme) => {
            app::with(|a| a.set_theme(theme));
            with_ui(|ui| {
                if ui.settings.theme != theme {
                    ui.settings.theme = theme;
                    ui.set_palette();
                } else {
                    unsafe { InvalidateRect(ui.window, std::ptr::null(), 0) };
                }
            });
        }
        Deferred::Command(command) => run_command(command),
        Deferred::Commit(field) => commit(field),
    }
}

fn toggle(switch: Switch) {
    let Some(on) = with_ui(|ui| ui.switch_value(switch)) else { return };
    match switch {
        Switch::History => {
            if with_ui(|ui| ui.policy_blocked).unwrap_or(false) {
                return;
            }
            change(|s| {
                s.history_enabled = !on;
                if !on {
                    s.paused = false;
                }
            })
        }
        Switch::Pause => change(|s| s.paused = !on),
        Switch::Paste => change(|s| s.paste_on_select = !on),
        Switch::SavePinned => change(|s| s.save_pinned = !on),
        Switch::KeepHistory => change(|s| s.keep_history = !on),
        Switch::Group => change(|s| s.group_duplicates = !on),
        Switch::Autostart => {
            if let Err(error) = system::set_autostart(!on) {
                message_box(owner(), &error, wm::MB_ICONWARNING);
            }
            with_ui(|ui| ui.refresh_from_app(false));
        }
        Switch::WinV => set_win_v(!on, owner(), true),
    }
}

/// Moves Win+V between Windows and BetterClipboard. Used by the settings switch and setup.
pub fn set_win_v(take: bool, owner: HWND, ask_restart: bool) {
    if let Err(error) = system::set_windows_win_v_disabled(take) {
        message_box(owner, &error, wm::MB_ICONWARNING);
        return;
    }
    let hotkey = if take { Hotkey::WIN_V } else { Hotkey::DEFAULT };
    change(|s| s.hotkey = hotkey);
    let restart = !ask_restart
        || message_box(
            owner,
            "Explorer restarts once so the change applies without signing out. Open File Explorer windows close.\n\nRestart Explorer now?",
            wm::MB_YESNO | wm::MB_ICONQUESTION,
        ) == wm::IDYES;
    if restart {
        // The taskbar comes back, BetterClipboard registers the shortcut again on TaskbarCreated.
        std::thread::spawn(system::restart_explorer);
    }
}

fn run_command(command: Command) {
    match command {
        Command::PhoneLink => {
            if !system::open("ms-phone:") {
                system::open("ms-settings:mobile-devices");
            }
        }
        Command::WindowsSettings => {
            system::open("ms-settings:clipboard");
        }
        Command::Snippet => dialogs::open_snippet(),
        Command::Clear => {
            let answer = message_box(
                owner(),
                "Clear clipboard data?\n\nThis removes every item that is not pinned, from memory and from disk, and empties the current clipboard.",
                wm::MB_YESNO | wm::MB_ICONQUESTION,
            );
            if answer == wm::IDYES {
                app::with(|a| a.clear_all(true));
            }
        }
    }
}

/// Validates a text field and applies it, or shows the problem under the setting.
fn commit(field: Field) {
    let Some((window, current)) = with_ui(|ui| (ui.window, ui.settings.clone())) else { return };
    let text = text_of(window, field.id());
    let number = |min: u32, max: u32, what: &str| -> Result<u32, String> {
        text.trim()
            .parse::<u32>()
            .ok()
            .filter(|v| (min..=max).contains(v))
            .ok_or(format!("{what} must be a number from {min} to {max}."))
    };
    let result: Result<Settings, String> = (|| {
        let mut next = current.clone();
        match field {
            Field::Hotkey => {
                next.hotkey = Hotkey::parse(&text).ok_or("Write the shortcut like Win+Alt+V or Ctrl+Shift+Space.")?;
            }
            Field::MaxItems => next.max_items = number(1, MAX_ITEMS_LIMIT, "Items to keep")?,
            Field::MaxMb => next.max_item_mb = number(1, MAX_ITEM_MB_LIMIT, "The largest item")?,
            Field::Retention => next.retention_days = number(0, 3650, "Days")?,
            Field::Excluded => {
                next.excluded_apps = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| l.rsplit('\\').next().unwrap_or(l).to_string())
                    .collect();
            }
        }
        Ok(next)
    })();
    match result {
        Ok(next) => {
            with_ui(|ui| ui.errors.retain(|(f, _)| *f != field));
            if next != current {
                app::with(|a| a.apply_settings(next));
            }
            with_ui(|ui| ui.refresh_from_app(true));
        }
        Err(message) => {
            with_ui(|ui| {
                ui.errors.retain(|(f, _)| *f != field);
                ui.errors.push((field, message));
                ui.layout();
            });
        }
    }
}

fn close() {
    with_ui(|ui| unsafe { wm::ShowWindow(ui.window, wm::SW_HIDE) });
}

pub fn destroy() {
    if let Some(ui) = UI.with(|cell| cell.try_borrow_mut().ok()?.take()) {
        unsafe {
            wm::DestroyWindow(ui.window);
            DeleteObject(ui.field_brush);
            SelectObject(ui.canvas.hdc, GetStockObject(DEFAULT_GUI_FONT));
            if !ui.icon.is_null() {
                wm::DestroyIcon(ui.icon);
            }
        }
        ui.fonts.release();
    }
}

fn commit_focused_field() {
    let field = with_ui(|ui| ui.edits.iter().find(|(_, e)| *e == unsafe { GetFocus() }).map(|(f, _)| *f)).flatten();
    if let Some(field) = field {
        commit(field);
    }
}

/// Keyboard handling for the settings window, called from the message loop before
/// dispatching. Returns true when the message was handled.
pub fn pretranslate(message: &wm::MSG) -> bool {
    let Some(window) = with_ui(|ui| ui.window) else { return false };
    let inside = message.hwnd == window || unsafe { wm::IsChild(window, message.hwnd) } != 0;
    if !inside || message.message != wm::WM_KEYDOWN {
        return false;
    }
    let vk = message.wParam as u16;
    let in_edit = message.hwnd != window;
    let multiline = in_edit
        && with_ui(|ui| ui.edits.iter().any(|(f, e)| *f == Field::Excluded && *e == message.hwnd)).unwrap_or(false);
    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
    match vk {
        0x09 => {
            // Tab
            let field = with_ui(|ui| ui.edits.iter().find(|(_, e)| *e == message.hwnd).map(|(f, _)| *f)).flatten();
            if let Some(field) = field {
                commit(field);
            }
            with_ui(|ui| ui.move_focus(shift));
            true
        }
        0x0D if in_edit && !multiline => {
            // Enter in a single-line field applies it.
            let field = with_ui(|ui| ui.edits.iter().find(|(_, e)| *e == message.hwnd).map(|(f, _)| *f)).flatten();
            if let Some(field) = field {
                commit(field);
            }
            true
        }
        0x1B => {
            close();
            true
        }
        0x0D | 0x20 if !in_edit => {
            let focus = with_ui(|ui| ui.focus).flatten();
            match focus {
                Some(Control::Back) => run(Deferred::Back),
                Some(Control::Switch(s)) => run(Deferred::Toggle(s)),
                Some(Control::Theme(theme)) => run(Deferred::Theme(theme)),
                Some(Control::Button(c)) => run(Deferred::Command(c)),
                _ => {}
            }
            true
        }
        0x25..=0x28 if !in_edit => {
            if let Some(Control::Theme(theme)) = with_ui(|ui| ui.focus).flatten() {
                let next = adjacent_theme(theme, vk);
                if next != theme {
                    with_ui(|ui| ui.focus = Some(Control::Theme(next)));
                    run(Deferred::Theme(next));
                }
                true
            } else {
                false
            }
        }
        0x21 | 0x22 if !in_edit => {
            let page = with_ui(|ui| ui.client().1 * 4 / 5).unwrap_or(400);
            with_ui(|ui| ui.scroll_by(if vk == 0x21 { -page } else { page }));
            true
        }
        _ => false,
    }
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            with_ui(|ui| ui.paint(dc));
            EndPaint(hwnd, &ps);
            0
        }
        wm::WM_ERASEBKGND => 1,
        wm::WM_SIZE => {
            if wparam as u32 != wm::SIZE_MINIMIZED {
                with_ui(|ui| {
                    if ui.client() != (ui.canvas.w, ui.canvas.h) {
                        ui.layout();
                    }
                });
            }
            0
        }
        wm::WM_GETMINMAXINFO => {
            let info = &mut *(lparam as *mut wm::MINMAXINFO);
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            info.ptMinTrackSize = POINT { x: 520 * dpi / 96, y: 420 * dpi / 96 };
            0
        }
        wm::WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            let step = with_ui(|ui| ui.px(64.0)).unwrap_or(64);
            with_ui(|ui| ui.scroll_by(-delta * step / 120));
            0
        }
        wm::WM_MOUSEMOVE => {
            let (x, y) = lparam_point(lparam);
            with_ui(|ui| {
                let hover = ui.control_at(x, y);
                if hover != ui.hover {
                    ui.hover = hover;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            });
            let mut track = windows_sys::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<windows_sys::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
                dwFlags: windows_sys::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            windows_sys::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut track);
            0
        }
        windows_sys::Win32::UI::Controls::WM_MOUSELEAVE => {
            with_ui(|ui| {
                ui.hover = None;
                InvalidateRect(hwnd, std::ptr::null(), 0);
            });
            0
        }
        wm::WM_LBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            let deferred = with_ui(|ui| {
                let control = ui.control_at(x, y)?;
                ui.focus = Some(control);
                match control {
                    Control::Back => Some(Deferred::Back),
                    Control::Switch(s) => {
                        SetFocus(hwnd);
                        Some(Deferred::Toggle(s))
                    }
                    Control::Theme(theme) => {
                        SetFocus(hwnd);
                        Some(Deferred::Theme(theme))
                    }
                    Control::Button(c) => {
                        SetFocus(hwnd);
                        Some(Deferred::Command(c))
                    }
                    Control::Field(f) => {
                        ui.set_focus(Control::Field(f));
                        None
                    }
                }
            })
            .flatten();
            if let Some(d) = deferred {
                run(d);
            }
            0
        }
        wm::WM_COMMAND => {
            // EN_KILLFOCUS applies the field that was edited; EN_SETFOCUS marks it focused.
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            if let Some(field) = Field::ALL.into_iter().find(|f| f.id() == id) {
                match code {
                    0x0200 => run(Deferred::Commit(field)),
                    0x0100 => {
                        with_ui(|ui| {
                            ui.focus = Some(Control::Field(field));
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        });
                    }
                    _ => {}
                }
            }
            0
        }
        wm::WM_CTLCOLOREDIT => {
            let colors = with_ui(|ui| (ui.field_brush, ui.palette.field, ui.palette.text));
            match colors {
                Some((brush, back, text)) => {
                    let dc = wparam as HDC;
                    SetTextColor(dc, gfx::cr(text));
                    SetBkColor(dc, gfx::cr(back));
                    brush as LRESULT
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_SETTINGCHANGE | wm::WM_SYSCOLORCHANGE => {
            with_ui(|ui| {
                ui.theme_previews = theme::preview_palettes();
                ui.preview_art = None;
                ui.set_palette();
                ui.refresh_from_app(true);
            });
            0
        }
        wm::WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            let r = *(lparam as *const RECT);
            with_ui(|ui| {
                ui.scale = dpi as f32 / 96.0;
                let old_fonts = std::mem::replace(&mut ui.fonts, Fonts::new(ui.scale));
                ui.apply_fonts();
                SelectObject(ui.canvas.hdc, GetStockObject(DEFAULT_GUI_FONT));
                old_fonts.release();
                if !ui.icon.is_null() {
                    wm::DestroyIcon(ui.icon);
                }
                ui.icon = crate::tray::app_icon(ui.px(40.0));
                ui.preview_art = None;
                wm::SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    wm::SWP_NOZORDER | wm::SWP_NOACTIVATE,
                );
                ui.layout();
            });
            0
        }
        wm::WM_ACTIVATE => {
            // Settings may have changed from the tray or panel while this window was open.
            if (wparam & 0xFFFF) as u32 != wm::WA_INACTIVE {
                with_ui(|ui| ui.refresh_from_app(false));
            }
            wm::DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        wm::WM_CLOSE => {
            // Apply a field that still has the focus before closing.
            commit_focused_field();
            close();
            0
        }
        _ => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod theme_picker_tests {
    use super::*;

    #[test]
    fn arrow_keys_reach_all_custom_themes() {
        let space = adjacent_theme(Theme::System, 0x28);
        let paper = adjacent_theme(space, 0x27);
        let candy = adjacent_theme(paper, 0x27);
        assert_eq!((space, paper, candy), (Theme::DeepSpace, Theme::PaperWhite, Theme::CandyRush));
        let frost = adjacent_theme(space, 0x28);
        let eclipse = adjacent_theme(paper, 0x28);
        let cyber = adjacent_theme(candy, 0x28);
        assert_eq!((frost, eclipse, cyber), (Theme::Frost, Theme::Eclipse, Theme::CyberGrid));
        assert_eq!(adjacent_theme(cyber, 0x28), cyber);
        assert_eq!(adjacent_theme(cyber, 0x27), cyber);
        assert_eq!(adjacent_theme(cyber, 0x25), eclipse);
        assert_eq!(adjacent_theme(cyber, 0x26), candy);
    }
}
