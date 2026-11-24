//! The clipboard history panel: a borderless, always-on-top flyout like Win+V.
//!
//! The window is created hidden at start-up and only shown, positioned and painted when
//! the hotkey arrives, which keeps the time to a usable panel short.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use super::gfx::{self, make_font, mix, Canvas, DT_C, DT_L, DT_MULTI};
use super::theme::{self, Palette};
use crate::model::{app_label, Filter, History, Item, Kind, Origin};
use crate::settings::Theme;
use crate::util::{self, wide};

pub const WIDTH_DIP: f32 = 360.0;
pub const HEIGHT_DIP: f32 = 520.0;

const GLYPH_SEARCH: &str = "\u{E721}";
const GLYPH_PIN: &str = "\u{E718}";
const GLYPH_UNPIN: &str = "\u{E77A}";
const GLYPH_DELETE: &str = "\u{E74D}";
const GLYPH_MORE: &str = "\u{E712}";
const GLYPH_SETTINGS: &str = "\u{E713}";
const GLYPH_SAVE: &str = "\u{E74E}";
const GLYPH_PHONE: &str = "\u{E8EA}";
const GLYPH_FOLDER: &str = "\u{E8B7}";
const GLYPH_PAUSE: &str = "\u{E769}";
const GLYPH_PINNED: &str = "\u{E840}";
const GLYPH_CLOSE: &str = "\u{E711}";
const GLYPH_CLIPBOARD: &str = "\u{E77F}";

/// The tab row: recently used, emoji, kaomoji, symbols and the clipboard history.
const TABS: [(&str, bool); 5] =
    [("\u{EB51}", true), ("\u{E76E}", true), (";-)", false), ("\u{03A9}", false), ("\u{E77F}", true)];
const FAVOURITES_TAB: usize = 0;
const EMOJI_TAB: usize = 1;
const KAOMOJI_TAB: usize = 2;
const SYMBOLS_TAB: usize = 3;
const CLIPBOARD_TAB: usize = 4;
const PAGE_TITLES: [&str; 5] = ["Recently used", "Emoji", "Kaomoji", "Symbols", "Clipboard"];

/// What the panel needs from the rest of the app to draw and react.
pub struct Context<'a> {
    pub history: &'a History,
    /// Emoji, kaomoji and symbols inserted recently, newest first (the favourites page).
    pub recent: &'a [String],
    pub history_enabled: bool,
    pub paused: bool,
    pub policy_blocked: bool,
    pub notice: Option<&'a str>,
    pub now: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Save,
    Delete,
    Pin,
    More,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Search,
    Chip(usize),
    ClearAll,
    Settings,
    Close,
    Tab(usize),
    Grabber,
    Cell(usize),
    Primary,
    Card(usize),
    CardButton(usize, Button),
    Scrollbar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Search,
    Chips,
    List,
    CardButton(Button),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PasteMode {
    /// Paste with all formats (or only copy, if the user turned pasting off).
    Default,
    PlainText,
    CopyOnly,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Hide,
    Choose {
        id: u64,
        mode: PasteMode,
    },
    Menu {
        id: u64,
        x: i32,
        y: i32,
    },
    Delete(u64),
    Pin(u64, bool),
    SaveImage(u64),
    ClearAll,
    OpenSettings,
    /// Type an emoji, kaomoji or symbol into the previous app.
    Insert(String),
    /// The button in the empty state: turn history on, or resume capture.
    Primary,
}

struct Card {
    id: u64,
    y: i32,
    h: i32,
    preview: String,
    image: Option<(i32, i32)>,
    footer: String,
    phone: bool,
    pinned: bool,
    has_image: bool,
}

struct Fonts {
    title: HFONT,
    body: HFONT,
    small: HFONT,
    chip: HFONT,
    icon: HFONT,
    icon_small: HFONT,
    icon_large: HFONT,
    tab_text: HFONT,
    emoji: HFONT,
    symbol: HFONT,
}

impl Fonts {
    fn new(scale: f32) -> Fonts {
        let px = |dip: f32| (dip * scale).round() as i32;
        let face = if gfx::font_exists("Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
        let icons = if gfx::font_exists("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        Fonts {
            title: make_font(face, px(15.0), 600),
            body: make_font(face, px(14.0), 400),
            small: make_font(face, px(12.0), 400),
            chip: make_font(face, px(12.0), 400),
            icon: make_font(icons, px(16.0), 400),
            icon_small: make_font(icons, px(12.0), 400),
            icon_large: make_font(icons, px(44.0), 400),
            emoji: make_font("Segoe UI Emoji", px(22.0), 400),
            symbol: make_font(face, px(20.0), 400),
            tab_text: make_font(face, px(12.0), 600),
        }
    }

    fn release(&self) {
        for f in [
            self.title,
            self.body,
            self.small,
            self.chip,
            self.icon,
            self.icon_small,
            self.icon_large,
            self.tab_text,
            self.emoji,
            self.symbol,
        ] {
            unsafe { DeleteObject(f) };
        }
    }
}

pub struct Popup {
    pub hwnd: HWND,
    canvas: Canvas,
    fonts: Fonts,
    scale: f32,
    palette: Palette,
    theme: Theme,
    pub visible: bool,
    /// Set while a menu or dialog owned by the panel is open, so losing focus does not hide it.
    pub modal: bool,
    pub query: String,
    caret: usize,
    pub filter: Filter,
    focus: Focus,
    selected: usize,
    cards: Vec<Card>,
    built_for: Option<(u64, String, Filter, i32, usize, usize)>,
    /// The page of the tab row that is shown (CLIPBOARD_TAB for the history).
    page: usize,
    /// Characters of an emoji, kaomoji, symbol or favourites page.
    cells: Vec<String>,
    /// Whether each cell is an emoji (drawn from the pre-rendered colour bitmaps).
    cell_is_emoji: Vec<bool>,
    cols: usize,
    cell: (i32, i32),
    content_h: i32,
    scroll: i32,
    hover: Hit,
    pressed: Hit,
    drag: Option<(i32, i32)>,
    toast: Option<(String, i64)>,
    /// The entry whose "See more" button was opened (shows pin and delete, like Win+V).
    expanded: Option<u64>,
    /// QPC time of the last completed paint (benchmarks read it).
    pub painted_at: f64,
    /// Milliseconds the last paint took (benchmarks read it).
    pub paint_ms: f64,
}

pub fn register_class(proc_: wm::WNDPROC) -> Vec<u16> {
    let class = wide(&format!("BetterClipboard.Panel{}", util::profile()));
    unsafe {
        let mut definition: wm::WNDCLASSEXW = std::mem::zeroed();
        definition.cbSize = std::mem::size_of_val(&definition) as u32;
        definition.hInstance = GetModuleHandleW(std::ptr::null());
        definition.lpszClassName = class.as_ptr();
        definition.lpfnWndProc = proc_;
        definition.style = wm::CS_DROPSHADOW;
        definition.hCursor = wm::LoadCursorW(std::ptr::null_mut(), wm::IDC_ARROW);
        wm::RegisterClassExW(&definition);
    }
    class
}

impl Popup {
    pub fn create(class: &[u16], theme: Theme) -> Popup {
        let hwnd = unsafe {
            wm::CreateWindowExW(
                wm::WS_EX_TOOLWINDOW | wm::WS_EX_TOPMOST,
                class.as_ptr(),
                wide("Clipboard history").as_ptr(),
                wm::WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        unsafe {
            let round = 2u32; // DWMWCP_ROUND
            DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &round as *const u32 as _, 4);
        }
        let scale = 1.0;
        Popup {
            hwnd,
            canvas: Canvas::new(),
            fonts: Fonts::new(scale),
            scale,
            palette: theme::palette(theme),
            theme,
            visible: false,
            modal: false,
            query: String::new(),
            caret: 0,
            filter: Filter::All,
            focus: Focus::List,
            selected: 0,
            cards: Vec::new(),
            built_for: None,
            content_h: 0,
            scroll: 0,
            hover: Hit::None,
            pressed: Hit::None,
            drag: None,
            toast: None,
            expanded: None,
            page: CLIPBOARD_TAB,
            cells: Vec::new(),
            cell_is_emoji: Vec::new(),
            cols: 8,
            cell: (1, 1),
            painted_at: 0.0,
            paint_ms: 0.0,
        }
    }

    fn px(&self, dip: f32) -> i32 {
        (dip * self.scale).round() as i32
    }

    fn width(&self) -> i32 {
        self.px(WIDTH_DIP)
    }

    fn height(&self) -> i32 {
        self.px(HEIGHT_DIP)
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.palette = theme::palette(theme);
        let border = gfx::cr(self.palette.border);
        unsafe {
            DwmSetWindowAttribute(self.hwnd, DWMWA_BORDER_COLOR as u32, &border as *const u32 as _, 4);
        }
        self.invalidate();
    }

    fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() > 0.001 {
            self.fonts.release();
            self.scale = scale;
            self.fonts = Fonts::new(scale);
            self.built_for = None;
        }
    }

    pub fn invalidate(&self) {
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    /// Shows the panel near `anchor` (the text caret, or the mouse). `below` is the line
    /// height under the anchor point, so the panel does not cover the caret line.
    pub fn show(&mut self, anchor: POINT, below: i32, from_tray: bool, ctx: &Context) {
        let monitor = unsafe { MonitorFromPoint(anchor, MONITOR_DEFAULTTONEAREST) };
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        self.set_scale(dpi_x as f32 / 96.0);
        self.palette = theme::palette(self.theme);
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        unsafe { GetMonitorInfoW(monitor, &mut info) };
        let work = info.rcWork;
        let (w, h) = (self.width(), self.height().min(work.bottom - work.top));
        let margin = self.px(8.0);
        let (x, y) = if from_tray {
            (work.right - w - margin, work.bottom - h - margin)
        } else {
            let mut y = anchor.y + below + margin / 2;
            if y + h > work.bottom {
                // Not enough room below the caret: open above it.
                y = (anchor.y - h - margin / 2).max(work.top);
            }
            (anchor.x.clamp(work.left + margin, (work.right - w - margin).max(work.left)), y)
        };

        self.query.clear();
        self.caret = 0;
        self.filter = Filter::All;
        self.focus = Focus::List;
        self.selected = 0;
        self.scroll = 0;
        self.hover = Hit::None;
        self.toast = None;
        self.expanded = None;
        self.page = CLIPBOARD_TAB;
        self.built_for = None;
        // Win+V always opens at the newest entry.
        self.cards.clear();
        self.rebuild(ctx);
        self.canvas.ensure(w, h);
        self.visible = true;
        let border = gfx::cr(self.palette.border);
        unsafe {
            DwmSetWindowAttribute(self.hwnd, DWMWA_BORDER_COLOR as u32, &border as *const u32 as _, 4);
            wm::SetWindowPos(self.hwnd, wm::HWND_TOPMOST, x, y, w, h, wm::SWP_SHOWWINDOW);
            wm::SetForegroundWindow(self.hwnd);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
        self.invalidate();
        self.announce(ctx);
    }

    pub fn hide(&mut self) {
        if self.visible {
            self.visible = false;
            self.modal = false;
            unsafe { wm::ShowWindow(self.hwnd, wm::SW_HIDE) };
        }
    }

    pub fn show_toast(&mut self, text: &str, now: i64) {
        self.toast = Some((text.to_string(), now + 2500));
        self.invalidate();
    }

    pub fn on_dpi_changed(&mut self, dpi: u32, suggested: &RECT, ctx: &Context) {
        self.set_scale(dpi as f32 / 96.0);
        unsafe {
            wm::SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                wm::SWP_NOZORDER | wm::SWP_NOACTIVATE,
            );
        }
        self.rebuild(ctx);
        self.invalidate();
    }

    // ------------------------------------------------------------------ layout

    fn close_rect(&self) -> (i32, i32, i32, i32) {
        (self.width() - self.px(8.0 + 32.0), self.px(4.0), self.px(32.0), self.px(28.0))
    }
    fn settings_rect(&self) -> (i32, i32, i32, i32) {
        (self.width() - self.px(8.0 + 32.0 + 2.0 + 32.0), self.px(4.0), self.px(32.0), self.px(28.0))
    }
    fn tab_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        (0..TABS.len() as i32)
            .map(|i| (self.px(12.0) + i * self.px(44.0), self.px(34.0), self.px(40.0), self.px(36.0)))
            .collect()
    }
    /// The grip at the top moves the panel, like the Windows one.
    pub fn is_grabber(&self, x: i32, y: i32) -> bool {
        let inside = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        y >= 0 && y < self.px(30.0) && !inside(self.close_rect()) && !inside(self.settings_rect())
    }
    fn section_y(&self) -> i32 {
        self.px(78.0)
    }
    fn search_rect(&self) -> (i32, i32, i32, i32) {
        (self.px(12.0), self.px(114.0), self.width() - self.px(24.0), self.px(32.0))
    }
    fn chips_y(&self) -> i32 {
        self.px(154.0)
    }
    fn banner_y(&self) -> i32 {
        self.px(190.0)
    }
    fn banner_h(&self, ctx: &Context) -> i32 {
        if self.page == CLIPBOARD_TAB && (ctx.paused || ctx.notice.is_some()) {
            self.px(40.0)
        } else {
            0
        }
    }
    fn list_top(&self, ctx: &Context) -> i32 {
        if self.page != CLIPBOARD_TAB {
            return self.chips_y();
        }
        self.px(190.0) + self.banner_h(ctx)
    }
    fn list_bottom(&self) -> i32 {
        self.height() - self.px(8.0)
    }
    fn card_x(&self) -> i32 {
        self.px(12.0)
    }
    fn card_w(&self) -> i32 {
        self.width() - self.px(24.0)
    }

    fn chip_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        let mut x = self.px(12.0);
        let y = self.chips_y();
        let h = self.px(26.0);
        let gap = self.px(6.0);
        let widths: Vec<i32> = Filter::ALL.iter().map(|f| self.canvas.text_width(self.fonts.chip, f.label())).collect();
        // Shrink the padding so all chips fit on one row, however wide the labels render.
        let available = self.width() - self.px(24.0) - gap * (widths.len() as i32 - 1);
        let text: i32 = widths.iter().sum();
        let pad = ((available - text) / widths.len() as i32).clamp(self.px(10.0), self.px(24.0));
        widths
            .into_iter()
            .map(|w| {
                let r = (x, y, w + pad, h);
                x += w + pad + gap;
                r
            })
            .collect()
    }

    fn clear_all_rect(&self) -> (i32, i32, i32, i32) {
        let w = self.canvas.text_width(self.fonts.body, "Clear all") + self.px(20.0);
        (self.width() - self.px(8.0) - w, self.section_y(), w, self.px(30.0))
    }

    /// Filtered entry indexes in history order.
    fn visible_items<'a>(&self, history: &'a History) -> Vec<&'a Item> {
        history.search(&self.query, self.filter).into_iter().map(|i| &history.items()[i]).collect()
    }

    pub fn rebuild(&mut self, ctx: &Context) {
        let key = (ctx.history.revision, self.query.clone(), self.filter, self.width(), self.page, ctx.recent.len());
        if self.built_for.as_ref() == Some(&key) {
            return;
        }
        if self.page != CLIPBOARD_TAB {
            self.build_cells(ctx);
            self.built_for = Some(key);
            return;
        }
        let previous = self.cards.get(self.selected).map(|c| c.id);
        self.canvas.ensure(self.width(), self.height());
        let inner_w = self.card_w() - self.px(24.0);
        let text_w = inner_w - self.px(32.0);
        let char_w = self.canvas.average_char_width(self.fonts.body).max(1);
        let per_line = (text_w / char_w).max(8) as usize;
        let line_h = self.px(20.0);
        let footer_h = self.px(18.0);
        let pad_top = self.px(12.0);
        let pad_bottom = self.px(10.0);
        let gap = self.px(8.0);
        let max_image_h = self.px(140.0);

        let mut cards = Vec::new();
        let mut y = 0;
        for item in self.visible_items(ctx.history) {
            let content = &item.content;
            let has_image = content.picture.is_some();
            let mut image = None;
            let (preview, lines) = if !content.files.is_empty() {
                let mut names: Vec<String> =
                    content.files.iter().take(3).map(|f| f.rsplit('\\').next().unwrap_or(f).to_string()).collect();
                if content.files.len() > 3 {
                    names.push(format!("and {} more", content.files.len() - 3));
                }
                let n = names.len() as i32;
                (names.join("\r\n"), n)
            } else if let (Some(picture), Kind::Image) = (&content.picture, content.kind()) {
                let (w, h) = crate::image::fit_size(picture.width, picture.height, inner_w as u32, max_image_h as u32);
                image = Some((w as i32, h as i32));
                (String::new(), 0)
            } else {
                preview_lines(content.text.as_deref().unwrap_or_default(), content, per_line)
            };
            let body_h = match image {
                Some((_, h)) => h,
                None => lines.max(1) * line_h,
            };
            let h = pad_top + body_h + self.px(6.0) + footer_h + pad_bottom;
            let mut footer =
                if item.origin == Origin::Phone { "From your phone".to_string() } else { app_label(&item.app) };
            if footer.is_empty() {
                footer = if item.origin == Origin::Snippet { "Snippet".into() } else { "Unknown app".into() };
            }
            footer += " \u{00B7} ";
            footer += &util::relative_age(item.last_copied_ms, ctx.now);
            if let Some(p) = &content.picture {
                footer += &format!(" \u{00B7} {}\u{00D7}{}", p.width, p.height);
            }
            if item.copies > 1 {
                footer += &format!(" \u{00B7} {}\u{00D7} copied", item.copies);
            }
            cards.push(Card {
                id: item.id,
                y,
                h,
                preview,
                image,
                footer,
                phone: item.origin == Origin::Phone,
                pinned: item.pinned,
                has_image,
            });
            y += h + gap;
        }
        self.content_h = (y - gap).max(0);
        self.cards = cards;
        self.built_for = Some(key);
        // Keep the same entry selected when the list changes under the cursor.
        self.selected = previous.and_then(|id| self.cards.iter().position(|c| c.id == id)).unwrap_or(0);
        if self.selected >= self.cards.len() {
            self.selected = self.cards.len().saturating_sub(1);
        }
        self.clamp_scroll_for(self.list_bottom() - self.px(128.0));
    }

    fn clamp_scroll_for(&mut self, viewport: i32) {
        let max = (self.content_h - viewport + self.px(8.0)).max(0);
        self.scroll = self.scroll.clamp(0, max);
    }

    fn viewport(&self, ctx: &Context) -> i32 {
        self.list_bottom() - self.list_top(ctx)
    }

    fn ensure_selected_visible(&mut self, ctx: &Context) {
        let Some(card) = self.cards.get(self.selected) else { return };
        let view = self.viewport(ctx);
        let (top, bottom) = (card.y, card.y + card.h);
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + view {
            self.scroll = bottom - view;
        }
        self.clamp_scroll_for(view);
    }

    fn card_buttons(&self, index: usize) -> Vec<Button> {
        let card = &self.cards[index];
        let mut buttons = vec![Button::More];
        if self.expanded == Some(card.id) {
            buttons.push(Button::Pin);
            buttons.push(Button::Delete);
            if card.has_image {
                buttons.push(Button::Save);
            }
        }
        buttons
    }

    /// Button rectangles right to left, in window coordinates.
    fn button_rect(&self, index: usize, slot: usize, ctx: &Context) -> (i32, i32, i32, i32) {
        let card = &self.cards[index];
        let size = self.px(28.0);
        let top = self.list_top(ctx) + card.y - self.scroll + self.px(6.0);
        let right = self.card_x() + self.card_w() - self.px(6.0);
        (right - size * (slot as i32 + 1) - self.px(2.0) * slot as i32, top, size, size)
    }

    pub fn hit(&self, x: i32, y: i32, ctx: &Context) -> Hit {
        let inside = |r: (i32, i32, i32, i32)| x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3;
        if inside(self.close_rect()) {
            return Hit::Close;
        }
        if inside(self.settings_rect()) {
            return Hit::Settings;
        }
        if self.is_grabber(x, y) {
            return Hit::Grabber;
        }
        for (i, r) in self.tab_rects().into_iter().enumerate() {
            if inside(r) {
                return Hit::Tab(i);
            }
        }
        if inside(self.clear_all_rect()) && ctx.history_enabled && self.page == CLIPBOARD_TAB {
            return Hit::ClearAll;
        }
        if (ctx.history_enabled || self.page != CLIPBOARD_TAB) && inside(self.search_rect()) {
            return Hit::Search;
        }
        if ctx.history_enabled && self.page == CLIPBOARD_TAB {
            for (i, r) in self.chip_rects().into_iter().enumerate() {
                if inside(r) {
                    return Hit::Chip(i);
                }
            }
        }
        if let Some(r) = self.primary_rect(ctx) {
            if inside(r) {
                return Hit::Primary;
            }
        }
        let (top, bottom) = (self.list_top(ctx), self.list_bottom());
        if y < top || y >= bottom {
            return Hit::None;
        }
        if self.content_h > bottom - top && x >= self.width() - self.px(10.0) {
            return Hit::Scrollbar;
        }
        let list_y = y - top + self.scroll;
        if self.page != CLIPBOARD_TAB {
            return self.cell_at(x, list_y);
        }
        if x < self.card_x() || x >= self.card_x() + self.card_w() {
            return Hit::None;
        }
        let Some(index) = self.cards.iter().position(|c| list_y >= c.y && list_y < c.y + c.h) else {
            return Hit::None;
        };
        for (slot, button) in self.card_buttons_for_hit(index).into_iter().enumerate() {
            if inside(self.button_rect(index, slot, ctx)) {
                return Hit::CardButton(index, button);
            }
        }
        Hit::Card(index)
    }

    /// Switches between the clipboard and the emoji, kaomoji and symbol pages.
    fn emoji_px(&self) -> i32 {
        self.px(24.0)
    }

    /// The colour emoji bitmaps for the current scale; starts rendering them if needed.
    fn emoji_sprites(&self) -> Option<super::emoji::Sprites> {
        let size = self.emoji_px();
        let sprites = super::emoji::sprites(size);
        if sprites.is_none() {
            let window = self.hwnd as isize;
            super::emoji::prepare(size, 0, move || unsafe {
                InvalidateRect(window as HWND, std::ptr::null(), 0);
            });
        }
        sprites
    }

    /// Renders the colour emoji for the main monitor in the background, so the emoji
    /// page is instant the first time it opens.
    pub fn prewarm_emoji(&self) {
        let dpi = unsafe {
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let (mut x, mut y) = (96u32, 96u32);
            GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
            x
        };
        let size = (24.0 * dpi as f32 / 96.0).round() as i32;
        super::emoji::prepare(size, 1500, || {});
    }

    /// Whether the colour emoji for the current scale are rendered (benchmarks).
    pub fn emoji_ready(&self) -> bool {
        super::emoji::sprites(self.emoji_px()).is_some()
    }

    /// Opens the emoji page (benchmarks).
    pub fn open_emoji_page(&mut self, ctx: &Context) {
        self.set_page(EMOJI_TAB, ctx);
    }

    fn set_page(&mut self, page: usize, ctx: &Context) {
        if self.page == page {
            return;
        }
        self.page = page;
        self.query.clear();
        self.caret = 0;
        self.selected = 0;
        self.scroll = 0;
        self.focus = Focus::List;
        self.built_for = None;
        self.rebuild(ctx);
        self.invalidate();
    }

    fn build_cells(&mut self, ctx: &Context) {
        let query = self.query.to_lowercase();
        let matches = |text: &str| query.is_empty() || text.to_lowercase().contains(&query);
        self.cells = match self.page {
            FAVOURITES_TAB => ctx.recent.iter().filter(|t| matches(t)).cloned().collect(),
            EMOJI_TAB => crate::symbols::EMOJI
                .iter()
                .filter(|(_, words)| query.split_whitespace().all(|w| words.contains(w)))
                .map(|(e, _)| e.to_string())
                .collect(),
            KAOMOJI_TAB => crate::symbols::KAOMOJI.iter().filter(|k| matches(k)).map(|k| k.to_string()).collect(),
            SYMBOLS_TAB => crate::symbols::SYMBOLS.iter().filter(|s| matches(s)).map(|s| s.to_string()).collect(),
            _ => Vec::new(),
        };
        let emoji: std::collections::HashSet<&str> = crate::symbols::EMOJI.iter().map(|(e, _)| *e).collect();
        self.cell_is_emoji = self.cells.iter().map(|c| emoji.contains(c.as_str())).collect();
        // Kaomoji are wide; everything else fits a square cell.
        let wide = self.cells.iter().any(|t| t.chars().count() > 3);
        let width = self.card_w();
        self.cols = if wide { 2 } else { 8 };
        self.cell = (width / self.cols as i32, if wide { self.px(40.0) } else { self.px(44.0) });
        let rows = self.cells.len().div_ceil(self.cols) as i32;
        self.content_h = rows * self.cell.1;
        self.selected = self.selected.min(self.cells.len().saturating_sub(1));
        let view = self.list_bottom() - self.list_top(ctx);
        self.clamp_scroll_for(view);
    }

    fn cell_at(&self, x: i32, list_y: i32) -> Hit {
        let (cell_w, cell_h) = self.cell;
        let col = (x - self.card_x()) / cell_w.max(1);
        if x < self.card_x() || col >= self.cols as i32 || list_y < 0 {
            return Hit::None;
        }
        let index = (list_y / cell_h.max(1)) as usize * self.cols + col as usize;
        if index < self.cells.len() {
            Hit::Cell(index)
        } else {
            Hit::None
        }
    }

    /// Arrow keys, Enter and Tab on the emoji, kaomoji and symbol pages.
    fn on_grid_key(&mut self, vk: u16, ctx: &Context) -> Option<Vec<Action>> {
        let cols = self.cols.max(1) as i32;
        let count = self.cells.len() as i32;
        let in_list = self.focus == Focus::List;
        let step = match vk {
            VK_LEFT if in_list => Some(-1),
            VK_RIGHT if in_list => Some(1),
            VK_UP => Some(-cols),
            VK_DOWN => Some(cols),
            _ => None,
        };
        if let Some(step) = step {
            if !in_list {
                if vk == VK_DOWN && count > 0 {
                    self.focus = Focus::List;
                    self.selected = 0;
                }
            } else if vk == VK_UP && (self.selected as i32) < cols {
                self.focus = Focus::Search;
                self.caret = self.query.len();
            } else if count > 0 {
                self.selected = (self.selected as i32 + step).clamp(0, count - 1) as usize;
                let (_, cell_h) = self.cell;
                let row_y = (self.selected as i32 / cols) * cell_h;
                let view = self.list_bottom() - self.list_top(ctx);
                if row_y < self.scroll {
                    self.scroll = row_y;
                } else if row_y + cell_h > self.scroll + view {
                    self.scroll = row_y + cell_h - view;
                }
            }
            self.invalidate();
            return Some(Vec::new());
        }
        match vk {
            VK_RETURN if in_list || count > 0 => {
                Some(self.cells.get(self.selected).map(|t| vec![Action::Insert(t.clone())]).unwrap_or_default())
            }
            VK_TAB => {
                self.focus = if in_list { Focus::Search } else { Focus::List };
                self.invalidate();
                Some(Vec::new())
            }
            _ => None,
        }
    }

    fn card_buttons_for_hit(&self, index: usize) -> Vec<Button> {
        self.card_buttons(index)
    }

    fn primary_rect(&self, ctx: &Context) -> Option<(i32, i32, i32, i32)> {
        let label = self.primary_label(ctx)?;
        let w = self.canvas.text_width(self.fonts.body, label) + self.px(32.0);
        if self.page == CLIPBOARD_TAB && ctx.paused && ctx.history_enabled && !ctx.policy_blocked {
            // The "Resume" button sits inside the banner.
            let banner_y = self.banner_y();
            return Some((
                self.width() - self.px(16.0) - w + self.px(8.0),
                banner_y + self.px(6.0),
                w - self.px(8.0),
                self.px(28.0),
            ));
        }
        let y = self.list_top(ctx) + self.px(176.0);
        let w = w.max(self.px(104.0));
        Some(((self.width() - w) / 2, y, w, self.px(32.0)))
    }

    fn primary_label(&self, ctx: &Context) -> Option<&'static str> {
        if self.page != CLIPBOARD_TAB {
            return None;
        }
        if ctx.policy_blocked {
            None
        } else if !ctx.history_enabled {
            Some("Turn on")
        } else if ctx.paused {
            Some("Resume")
        } else {
            None
        }
    }

    // ------------------------------------------------------------------ input

    fn selected_id(&self) -> Option<u64> {
        self.cards.get(self.selected).map(|c| c.id)
    }

    fn selected_pinned(&self) -> bool {
        self.cards.get(self.selected).is_some_and(|c| c.pinned)
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32, ctx: &Context) {
        if let Some((start_y, start_scroll)) = self.drag {
            let view = self.viewport(ctx);
            let ratio = self.content_h as f32 / view.max(1) as f32;
            self.scroll = start_scroll + ((y - start_y) as f32 * ratio) as i32;
            self.clamp_scroll_for(view);
            self.invalidate();
            return;
        }
        let hit = self.hit(x, y, ctx);
        if hit != self.hover {
            self.hover = hit;
            self.invalidate();
        }
    }

    pub fn on_mouse_leave(&mut self) {
        if self.hover != Hit::None {
            self.hover = Hit::None;
            self.invalidate();
        }
    }

    pub fn on_button_down(&mut self, x: i32, y: i32, ctx: &Context) {
        self.pressed = self.hit(x, y, ctx);
        if self.pressed == Hit::Scrollbar {
            self.drag = Some((y, self.scroll));
            unsafe { SetCapture(self.hwnd) };
        }
        if self.pressed == Hit::Search {
            self.focus = Focus::Search;
            self.caret = self.query.len();
            self.invalidate();
        }
    }

    pub fn on_button_up(&mut self, x: i32, y: i32, ctx: &Context) -> Vec<Action> {
        if self.drag.take().is_some() {
            unsafe { ReleaseCapture() };
            return Vec::new();
        }
        let hit = self.hit(x, y, ctx);
        let pressed = std::mem::replace(&mut self.pressed, Hit::None);
        if hit != pressed {
            return Vec::new();
        }
        match hit {
            Hit::Settings => vec![Action::OpenSettings],
            Hit::Close => vec![Action::Hide],
            Hit::Tab(i) => {
                self.set_page(i, ctx);
                Vec::new()
            }
            Hit::Cell(i) => match self.cells.get(i) {
                Some(text) => vec![Action::Insert(text.clone())],
                None => Vec::new(),
            },
            Hit::ClearAll => vec![Action::ClearAll],
            Hit::Primary => vec![Action::Primary],
            Hit::Chip(i) => {
                self.set_filter(Filter::ALL[i], ctx);
                Vec::new()
            }
            Hit::Card(i) => {
                self.selected = i;
                vec![Action::Choose { id: self.cards[i].id, mode: PasteMode::Default }]
            }
            Hit::CardButton(i, button) => {
                self.selected = i;
                let card = &self.cards[i];
                match button {
                    Button::Pin => vec![Action::Pin(card.id, !card.pinned)],
                    Button::Delete => vec![Action::Delete(card.id)],
                    Button::Save => vec![Action::SaveImage(card.id)],
                    Button::More => {
                        // Like Win+V: "See more" reveals pin and delete on the entry.
                        self.expanded = if self.expanded == Some(card.id) { None } else { Some(card.id) };
                        self.invalidate();
                        Vec::new()
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    pub fn on_right_click(&mut self, x: i32, y: i32, ctx: &Context) -> Vec<Action> {
        match self.hit(x, y, ctx) {
            Hit::Card(i) | Hit::CardButton(i, _) => {
                self.selected = i;
                self.invalidate();
                let mut point = POINT { x, y };
                unsafe { ClientToScreen(self.hwnd, &mut point) };
                vec![Action::Menu { id: self.cards[i].id, x: point.x, y: point.y }]
            }
            _ => Vec::new(),
        }
    }

    pub fn on_wheel(&mut self, delta: i32, ctx: &Context) {
        let view = self.viewport(ctx);
        self.scroll -= delta * self.px(48.0) / 120;
        self.clamp_scroll_for(view);
        self.hover = Hit::None;
        self.invalidate();
    }

    fn set_filter(&mut self, filter: Filter, ctx: &Context) {
        if self.filter != filter {
            self.filter = filter;
            self.selected = 0;
            self.scroll = 0;
            self.rebuild(ctx);
            self.invalidate();
            self.announce(ctx);
        }
    }

    fn query_changed(&mut self, ctx: &Context) {
        self.selected = 0;
        self.scroll = 0;
        self.rebuild(ctx);
        self.invalidate();
    }

    fn move_selection(&mut self, delta: i32, ctx: &Context) {
        if self.cards.is_empty() {
            return;
        }
        let last = self.cards.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
        self.focus = Focus::List;
        self.ensure_selected_visible(ctx);
        self.invalidate();
        self.announce(ctx);
    }

    pub fn on_key(&mut self, vk: u16, ctx: &Context) -> Vec<Action> {
        let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
        let (ctrl, shift) = (down(VK_CONTROL), down(VK_SHIFT));
        let in_search = self.focus == Focus::Search;
        let page = (self.viewport(ctx) / self.px(80.0)).max(1);
        if self.page != CLIPBOARD_TAB {
            if let Some(actions) = self.on_grid_key(vk, ctx) {
                return actions;
            }
        }
        match vk {
            VK_ESCAPE => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.caret = 0;
                    self.query_changed(ctx);
                    return Vec::new();
                }
                return vec![Action::Hide];
            }
            VK_DOWN => self.move_selection(if self.focus == Focus::List { 1 } else { 0 }, ctx),
            VK_UP => {
                if self.focus == Focus::List && self.selected == 0 {
                    self.focus = Focus::Search;
                    self.invalidate();
                } else {
                    self.move_selection(-1, ctx);
                }
            }
            VK_NEXT => self.move_selection(page, ctx),
            VK_PRIOR => self.move_selection(-page, ctx),
            VK_HOME if !in_search => self.move_selection(-(self.cards.len() as i32), ctx),
            VK_END if !in_search => self.move_selection(self.cards.len() as i32, ctx),
            VK_HOME => {
                self.caret = 0;
                self.invalidate();
            }
            VK_END => {
                self.caret = self.query.len();
                self.invalidate();
            }
            VK_TAB => {
                self.focus = match (self.focus, shift) {
                    (Focus::Search, false) => Focus::Chips,
                    (Focus::Chips, false) => Focus::List,
                    (Focus::List | Focus::CardButton(_), false) => Focus::Search,
                    (Focus::Search, true) => Focus::List,
                    (Focus::Chips, true) => Focus::Search,
                    (Focus::List | Focus::CardButton(_), true) => Focus::Chips,
                };
                self.invalidate();
            }
            VK_LEFT | VK_RIGHT => {
                let right = vk == VK_RIGHT;
                match self.focus {
                    Focus::Search => {
                        self.caret = if right {
                            next_boundary(&self.query, self.caret)
                        } else {
                            prev_boundary(&self.query, self.caret)
                        };
                    }
                    Focus::Chips => {
                        let i = Filter::ALL.iter().position(|f| *f == self.filter).unwrap_or(0) as i32;
                        let next = (i + if right { 1 } else { -1 }).clamp(0, Filter::ALL.len() as i32 - 1);
                        self.set_filter(Filter::ALL[next as usize], ctx);
                    }
                    Focus::List | Focus::CardButton(_) => {
                        if !self.cards.is_empty() {
                            let buttons = self.card_buttons_for_hit(self.selected);
                            // Buttons are stored right to left; arrow keys walk them left to right.
                            let order: Vec<Button> = buttons.iter().rev().copied().collect();
                            self.focus = match self.focus {
                                Focus::List if right => Focus::CardButton(order[0]),
                                Focus::CardButton(b) => {
                                    let pos = order.iter().position(|x| *x == b).unwrap_or(0);
                                    if right {
                                        Focus::CardButton(order[(pos + 1).min(order.len() - 1)])
                                    } else if pos == 0 {
                                        Focus::List
                                    } else {
                                        Focus::CardButton(order[pos - 1])
                                    }
                                }
                                other => other,
                            };
                        }
                    }
                }
                self.invalidate();
            }
            VK_RETURN | VK_SPACE if self.focus == Focus::Chips || matches!(self.focus, Focus::CardButton(_)) => {
                if let Focus::CardButton(button) = self.focus {
                    let Some(id) = self.selected_id() else { return Vec::new() };
                    return match button {
                        Button::Pin => vec![Action::Pin(id, !self.selected_pinned())],
                        Button::Delete => vec![Action::Delete(id)],
                        Button::Save => vec![Action::SaveImage(id)],
                        Button::More => {
                            self.expanded = if self.expanded == Some(id) { None } else { Some(id) };
                            self.invalidate();
                            Vec::new()
                        }
                    };
                }
                self.focus = Focus::List;
                self.invalidate();
            }
            VK_RETURN => {
                if let Some(id) = self.selected_id() {
                    let mode = if shift {
                        PasteMode::PlainText
                    } else if ctrl {
                        PasteMode::CopyOnly
                    } else {
                        PasteMode::Default
                    };
                    return vec![Action::Choose { id, mode }];
                }
            }
            VK_DELETE if !in_search => {
                if let Some(id) = self.selected_id() {
                    return vec![Action::Delete(id)];
                }
            }
            VK_DELETE => {
                if self.caret < self.query.len() {
                    let end = next_boundary(&self.query, self.caret);
                    self.query.replace_range(self.caret..end, "");
                    self.query_changed(ctx);
                }
            }
            VK_BACK if in_search || !self.query.is_empty() => {
                self.focus = Focus::Search;
                if ctrl {
                    let start = word_start(&self.query, self.caret);
                    self.query.replace_range(start..self.caret, "");
                    self.caret = start;
                } else if self.caret > 0 {
                    let start = prev_boundary(&self.query, self.caret);
                    self.query.replace_range(start..self.caret, "");
                    self.caret = start;
                }
                self.query_changed(ctx);
            }
            VK_APPS => {
                if let Some(id) = self.selected_id() {
                    let card = &self.cards[self.selected];
                    let mut point = POINT {
                        x: self.card_x() + self.px(24.0),
                        y: self.list_top(ctx) + card.y - self.scroll + self.px(24.0),
                    };
                    unsafe { ClientToScreen(self.hwnd, &mut point) };
                    return vec![Action::Menu { id, x: point.x, y: point.y }];
                }
            }
            _ if ctrl && vk == b'P' as u16 => {
                if let Some(id) = self.selected_id() {
                    return vec![Action::Pin(id, !self.selected_pinned())];
                }
            }
            _ if ctrl && vk == b'S' as u16 => {
                if let Some(card) = self.cards.get(self.selected).filter(|c| c.has_image) {
                    return vec![Action::SaveImage(card.id)];
                }
            }
            _ if ctrl && vk == b'F' as u16 => {
                self.focus = Focus::Search;
                self.caret = self.query.len();
                self.invalidate();
            }
            _ if ctrl && vk == b'A' as u16 && in_search => {
                self.caret = self.query.len();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Typed characters go to the search box, wherever the focus is ("type to search").
    pub fn on_char(&mut self, c: char, ctx: &Context) {
        if c.is_control() {
            return;
        }
        if self.focus != Focus::Search {
            self.focus = Focus::Search;
            self.caret = self.query.len();
        }
        if self.query.chars().count() >= 200 {
            return;
        }
        self.query.insert(self.caret, c);
        self.caret += c.len_utf8();
        self.query_changed(ctx);
    }

    /// Screen readers get the selected entry through the window title.
    fn announce(&self, ctx: &Context) {
        let title = match self.cards.get(self.selected) {
            Some(card) => {
                let what = if card.image.is_some() {
                    "Image".to_string()
                } else {
                    card.preview.lines().next().unwrap_or("").chars().take(80).collect()
                };
                format!(
                    "Clipboard history, entry {} of {}: {}. {}",
                    self.selected + 1,
                    self.cards.len(),
                    what,
                    card.footer
                )
            }
            None if !ctx.history_enabled => "Clipboard history is turned off".to_string(),
            None => "Clipboard history, no entries".to_string(),
        };
        unsafe {
            wm::SetWindowTextW(self.hwnd, wide(&title).as_ptr());
            windows_sys::Win32::UI::Accessibility::NotifyWinEvent(
                wm::EVENT_OBJECT_NAMECHANGE,
                self.hwnd,
                wm::OBJID_WINDOW,
                wm::CHILDID_SELF as i32,
            );
        }
    }

    // ------------------------------------------------------------------ painting

    pub fn paint(&mut self, dc: HDC, ctx: &Context) {
        let started = util::qpc_seconds();
        self.rebuild(ctx);
        let (w, h) = (self.width(), self.height());
        self.canvas.ensure(w, h);
        let p = self.palette;
        let clear_r = self.clear_all_rect();
        let settings_r = self.settings_rect();
        let close_r = self.close_rect();
        let tabs = self.tab_rects();
        let section_y = self.section_y();
        let banner_y = self.banner_y();
        let (sx, sy, sw, sh) = self.search_rect();
        let chips = if self.page == CLIPBOARD_TAB { self.chip_rects() } else { Vec::new() };
        let page_title = PAGE_TITLES[self.page];
        let emoji_sprites = if self.page == CLIPBOARD_TAB { None } else { self.emoji_sprites() };
        let placeholder = match self.page {
            EMOJI_TAB => "Search emoji",
            KAOMOJI_TAB => "Search kaomoji",
            SYMBOLS_TAB => "Search symbols",
            FAVOURITES_TAB => "Search recently used",
            _ => "Search clipboard history",
        };
        let banner = self.banner_h(ctx);
        let (top, bottom) = (self.list_top(ctx), self.list_bottom());
        let cx = self.card_x();
        let cw = self.card_w();
        let primary = self.primary_rect(ctx).zip(self.primary_label(ctx));
        let c = &mut self.canvas;
        c.fill(0, 0, w, h, p.background);

        let px = |dip: f32| (dip * self.scale).round() as i32;
        let pxf = |dip: f32| dip * self.scale;

        // Grip, settings and close, as on the Windows panel.
        c.round_rect((w / 2 - px(16.0)) as f32, pxf(10.0), pxf(32.0), pxf(4.0), pxf(2.0), p.tertiary, 1.0);
        let header_button = |c: &mut Canvas, r: (i32, i32, i32, i32), hot: bool| {
            if hot {
                c.round_rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32, pxf(4.0), p.control_hover, 1.0);
            }
        };
        header_button(c, settings_r, self.hover == Hit::Settings);
        c.text(self.fonts.icon_small, GLYPH_SETTINGS, settings_r, p.text, DT_C);
        header_button(c, close_r, self.hover == Hit::Close);
        c.text(self.fonts.icon_small, GLYPH_CLOSE, close_r, p.text, DT_C);

        // Tab row: recently used, emoji, kaomoji, symbols, clipboard.
        for (i, (r, (label, glyph))) in tabs.iter().zip(TABS).enumerate() {
            header_button(c, *r, self.hover == Hit::Tab(i));
            let font = if glyph { self.fonts.icon } else { self.fonts.tab_text };
            c.text(font, label, *r, p.text, DT_C);
            if i == self.page {
                let (ux, uy) = (r.0 + r.2 / 2 - px(8.0), r.1 + r.3 - px(3.0));
                c.round_rect(ux as f32, uy as f32, pxf(16.0), pxf(3.0), pxf(1.5), p.accent, 1.0);
            }
        }

        // Section title and "Clear all".
        c.text(self.fonts.body, page_title, (px(16.0), section_y, w / 2, px(30.0)), p.text, DT_L);
        if self.page == CLIPBOARD_TAB && ctx.history_enabled && !self.cards.is_empty() {
            header_button(c, clear_r, self.hover == Hit::ClearAll);
            c.text(self.fonts.body, "Clear all", clear_r, p.text, DT_C);
        }

        // Search box and filters (BetterClipboard additions), shown while history is on.
        if ctx.history_enabled || self.page != CLIPBOARD_TAB {
            let focused = self.focus == Focus::Search;
            c.round_rect(sx as f32, sy as f32, sw as f32, sh as f32, pxf(4.0), p.field, 1.0);
            c.round_rect_outline(sx as f32, sy as f32, sw as f32, sh as f32, pxf(4.0), 1.0, p.field_border, 1.0);
            if focused {
                c.fill(sx + px(1.0), sy + sh - px(2.0), sw - px(2.0), px(2.0), p.accent);
            }
            c.text(self.fonts.icon_small, GLYPH_SEARCH, (sx + px(10.0), sy, px(16.0), sh), p.secondary, DT_C);
            let text_x = sx + px(34.0);
            if self.query.is_empty() {
                c.text(self.fonts.body, placeholder, (text_x, sy, sw - px(44.0), sh), p.tertiary, DT_L);
            } else {
                c.text(self.fonts.body, &self.query, (text_x, sy, sw - px(44.0), sh), p.text, DT_L);
            }
            if focused {
                let caret_x = text_x + c.text_width(self.fonts.body, &self.query[..self.caret]);
                c.fill(caret_x.min(sx + sw - px(10.0)), sy + px(8.0), px(1.0).max(1), sh - px(16.0), p.text);
            }

            // Filter chips.
            for (i, (r, filter)) in chips.into_iter().zip(Filter::ALL).enumerate() {
                let active = self.filter == filter;
                let hot = self.hover == Hit::Chip(i);
                let (fill, text) = if active {
                    (p.accent, p.on_accent)
                } else if hot {
                    (p.control_hover, p.text)
                } else {
                    (p.card, p.text)
                };
                c.round_rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32, r.3 as f32 / 2.0, fill, 1.0);
                if !active {
                    c.round_rect_outline(
                        r.0 as f32,
                        r.1 as f32,
                        r.2 as f32,
                        r.3 as f32,
                        r.3 as f32 / 2.0,
                        1.0,
                        p.card_border,
                        1.0,
                    );
                }
                if self.focus == Focus::Chips && active {
                    c.round_rect_outline(
                        r.0 as f32 - pxf(2.0),
                        r.1 as f32 - pxf(2.0),
                        r.2 as f32 + pxf(4.0),
                        r.3 as f32 + pxf(4.0),
                        r.3 as f32 / 2.0 + pxf(2.0),
                        pxf(2.0),
                        p.focus,
                        1.0,
                    );
                }
                c.text(self.fonts.chip, filter.label(), r, text, DT_C);
            }
        }

        // Pause / notice banner.
        if banner > 0 {
            let y = banner_y - px(4.0);
            c.round_rect(
                px(12.0) as f32,
                y as f32,
                (w - px(24.0)) as f32,
                (banner - px(4.0)) as f32,
                pxf(6.0),
                p.banner,
                1.0,
            );
            let (glyph, text) = if ctx.paused {
                (GLYPH_PAUSE, "Recording is paused")
            } else {
                ("\u{E7BA}", ctx.notice.unwrap_or_default())
            };
            c.text(self.fonts.icon_small, glyph, (px(22.0), y, px(16.0), banner - px(4.0)), p.text, DT_C);
            c.text(self.fonts.small, text, (px(44.0), y, w - px(150.0), banner - px(4.0)), p.text, DT_L);
        }

        // List, or the grid of an emoji / kaomoji / symbol page.
        if self.page != CLIPBOARD_TAB {
            let (cell_w, cell_h) = self.cell;
            let cols = self.cols.max(1);
            c.clip_rect(Some((0, top, w, bottom - top)));
            let empty = if self.cells.is_empty() && self.page == FAVOURITES_TAB && self.query.is_empty() {
                Some(("Nothing used yet", "Emoji, kaomoji and symbols you insert show up here."))
            } else if self.cells.is_empty() {
                Some(("No matches", "Try another word."))
            } else {
                None
            };
            if let Some((title, body)) = empty {
                c.text(self.fonts.icon_large, "\u{EB51}", (0, top + px(28.0), w, px(56.0)), p.text, DT_C);
                c.text(self.fonts.title, title, (px(24.0), top + px(98.0), w - px(48.0), px(24.0)), p.text, DT_C);
                c.text(
                    self.fonts.body,
                    body,
                    (px(36.0), top + px(124.0), w - px(72.0), px(44.0)),
                    p.text,
                    DT_CENTER | DT_WORDBREAK,
                );
            }
            for (i, text) in self.cells.iter().enumerate() {
                let (col, row) = ((i % cols) as i32, (i / cols) as i32);
                let (x, y) = (cx + col * cell_w, top + row * cell_h - self.scroll);
                if y + cell_h < top || y > bottom {
                    continue;
                }
                let hot = self.hover == Hit::Cell(i);
                let selected = self.focus == Focus::List && self.selected == i;
                if hot || selected {
                    c.round_rect(
                        (x + px(2.0)) as f32,
                        (y + px(2.0)) as f32,
                        (cell_w - px(4.0)) as f32,
                        (cell_h - px(4.0)) as f32,
                        pxf(4.0),
                        p.control_hover,
                        1.0,
                    );
                }
                if selected {
                    c.round_rect_outline(
                        (x + px(2.0)) as f32,
                        (y + px(2.0)) as f32,
                        (cell_w - px(4.0)) as f32,
                        (cell_h - px(4.0)) as f32,
                        pxf(4.0),
                        pxf(2.0),
                        p.accent,
                        1.0,
                    );
                }
                if self.cell_is_emoji.get(i).copied().unwrap_or(false) {
                    // Colour emoji come pre-rendered from a background thread; until they
                    // are ready (only right after start), GDI draws outlines.
                    match emoji_sprites.as_ref().and_then(|s| s.get(text.as_str())) {
                        Some(sprite) => {
                            let (sx, sy) = (x + (cell_w - sprite.size) / 2, y + (cell_h - sprite.size) / 2);
                            c.blit_premultiplied(&sprite.pixels, sprite.size, sx, sy, top, bottom);
                        }
                        None => c.text(self.fonts.emoji, text, (x, y, cell_w, cell_h), p.text, DT_C),
                    }
                    continue;
                }
                // The emoji font lacks many symbols, so symbols use the text font.
                let font = if text.chars().count() > 3 { self.fonts.body } else { self.fonts.symbol };
                c.text(font, text, (x, y, cell_w, cell_h), p.text, DT_C);
            }
        } else {
            if self.cards.is_empty() {
                let (title, body) = if ctx.policy_blocked {
                    ("Clipboard history is off", "Your organization manages this setting.")
                } else if !ctx.history_enabled {
                    ("Let's get started", "Turn on clipboard history to copy and view multiple items.")
                } else if !self.query.is_empty() || self.filter != Filter::All {
                    ("No matches", "Try another word or filter.")
                } else {
                    ("You haven't copied anything yet", "Copy text, links, screenshots or files and they show up here.")
                };
                c.text(self.fonts.icon_large, GLYPH_CLIPBOARD, (0, top + px(28.0), w, px(56.0)), p.text, DT_C);
                c.text(self.fonts.title, title, (px(24.0), top + px(98.0), w - px(48.0), px(24.0)), p.text, DT_C);
                c.text(
                    self.fonts.body,
                    body,
                    (px(36.0), top + px(124.0), w - px(72.0), px(44.0)),
                    p.text,
                    DT_CENTER | DT_WORDBREAK,
                );
            }
            c.clip_rect(Some((0, top, w, bottom - top)));
            let line_h = px(20.0);
            for (i, card) in self.cards.iter().enumerate() {
                let y = top + card.y - self.scroll;
                if y + card.h < top {
                    continue;
                }
                if y > bottom {
                    break;
                }
                let hovered = matches!(self.hover, Hit::Card(k) | Hit::CardButton(k, _) if k == i);
                let selected = i == self.selected;
                let fill = if hovered { p.card_hover } else { p.card };
                c.round_rect(cx as f32, y as f32, cw as f32, card.h as f32, pxf(6.0), fill, 1.0);
                c.round_rect_outline(cx as f32, y as f32, cw as f32, card.h as f32, pxf(6.0), 1.0, p.card_border, 1.0);
                if selected && matches!(self.focus, Focus::List | Focus::CardButton(_)) {
                    c.round_rect_outline(
                        cx as f32,
                        y as f32,
                        cw as f32,
                        card.h as f32,
                        pxf(6.0),
                        pxf(2.0),
                        p.accent,
                        1.0,
                    );
                }
                let inner_x = cx + px(12.0);
                let inner_w = cw - px(24.0);
                let body_y = y + px(12.0);
                let body_h;
                if let (Some((iw, ih)), Some(item)) = (card.image, ctx.history.get(card.id)) {
                    if let Some(picture) = &item.content.picture {
                        // Checkerboard behind transparent pictures, like image viewers.
                        if !picture.thumb.is_opaque() {
                            let tile = px(6.0).max(2);
                            let mut ty = 0;
                            while ty < ih {
                                let mut tx = 0;
                                while tx < iw {
                                    let dark = ((tx / tile) + (ty / tile)) % 2 == 0;
                                    let shade = if dark { mix(fill, 0x808080, 0.25) } else { fill };
                                    c.fill(
                                        inner_x + tx,
                                        (body_y + ty).max(top),
                                        tile.min(iw - tx),
                                        tile.min(ih - ty),
                                        shade,
                                    );
                                    tx += tile;
                                }
                                ty += tile;
                            }
                        }
                        c.picture(&picture.thumb, inner_x, body_y, iw, ih, top, bottom);
                        c.round_rect_outline(
                            inner_x as f32,
                            body_y as f32,
                            iw as f32,
                            ih as f32,
                            pxf(2.0),
                            1.0,
                            p.card_border,
                            0.8,
                        );
                    }
                    body_h = ih;
                } else {
                    let lines = card.preview.lines().count().max(1) as i32;
                    body_h = lines * line_h;
                    let is_files = ctx.history.get(card.id).is_some_and(|it| !it.content.files.is_empty());
                    if is_files {
                        for (n, name) in card.preview.lines().enumerate() {
                            let ly = body_y + n as i32 * line_h;
                            c.text(
                                self.fonts.icon_small,
                                GLYPH_FOLDER,
                                (inner_x, ly, px(16.0), line_h),
                                p.secondary,
                                DT_C,
                            );
                            c.text(
                                self.fonts.body,
                                name,
                                (inner_x + px(22.0), ly, inner_w - px(54.0), line_h),
                                p.text,
                                DT_L,
                            );
                        }
                    } else {
                        c.text(
                            self.fonts.body,
                            &card.preview,
                            (inner_x, body_y, inner_w - px(32.0), body_h),
                            p.text,
                            DT_MULTI,
                        );
                    }
                }
                // Footer: source app, age; phone and pin markers.
                let fy = body_y + body_h + px(6.0);
                let mut fx = inner_x;
                if card.phone {
                    c.text(self.fonts.icon_small, GLYPH_PHONE, (fx, fy, px(14.0), px(18.0)), p.accent, DT_C);
                    fx += px(18.0);
                }
                c.text(
                    self.fonts.small,
                    &card.footer,
                    (fx, fy, inner_w - (fx - inner_x) - px(24.0), px(18.0)),
                    p.tertiary,
                    DT_L,
                );
                if card.pinned {
                    c.text(
                        self.fonts.icon_small,
                        GLYPH_PINNED,
                        (inner_x + inner_w - px(16.0), fy, px(16.0), px(18.0)),
                        p.secondary,
                        DT_C,
                    );
                }
            }

            // Card buttons (drawn after the cards so they sit on top).
            for i in 0..self.cards.len() {
                let card = &self.cards[i];
                let y = top + card.y - self.scroll;
                if y + card.h < top || y > bottom {
                    continue;
                }
                let hovered = matches!(self.hover, Hit::Card(k) | Hit::CardButton(k, _) if k == i);
                let fill = if hovered { p.card_hover } else { p.card };
                for (slot, button) in self.card_buttons(i).into_iter().enumerate() {
                    let r = self.button_rect(i, slot, ctx);
                    let hot = self.hover == Hit::CardButton(i, button);
                    let focused = i == self.selected && self.focus == Focus::CardButton(button);
                    let c = &mut self.canvas;
                    c.round_rect(
                        r.0 as f32,
                        r.1 as f32,
                        r.2 as f32,
                        r.3 as f32,
                        pxf(4.0),
                        if hot { p.control_hover } else { fill },
                        1.0,
                    );
                    if focused {
                        c.round_rect_outline(
                            r.0 as f32,
                            r.1 as f32,
                            r.2 as f32,
                            r.3 as f32,
                            pxf(4.0),
                            pxf(2.0),
                            p.focus,
                            1.0,
                        );
                    }
                    let glyph = match button {
                        Button::More => GLYPH_MORE,
                        Button::Pin if card.pinned => GLYPH_UNPIN,
                        Button::Pin => GLYPH_PIN,
                        Button::Delete => GLYPH_DELETE,
                        Button::Save => GLYPH_SAVE,
                    };
                    c.text(self.fonts.icon_small, glyph, r, p.text, DT_C);
                }
            }
        }
        let c = &mut self.canvas;
        c.clip_rect(None);

        // Scrollbar.
        let view = bottom - top;
        if self.content_h > view {
            let hot = matches!(self.hover, Hit::Scrollbar) || self.drag.is_some();
            let bar_w = if hot { px(6.0) } else { px(3.0) }.max(2);
            let thumb_h = ((view as f32 / self.content_h as f32) * view as f32).max(px(24.0) as f32) as i32;
            let max_scroll = (self.content_h - view + px(8.0)).max(1);
            let thumb_y = top + ((self.scroll as f32 / max_scroll as f32) * (view - thumb_h) as f32) as i32;
            c.round_rect(
                (w - px(4.0) - bar_w) as f32,
                thumb_y as f32,
                bar_w as f32,
                thumb_h as f32,
                bar_w as f32 / 2.0,
                p.tertiary,
                0.9,
            );
        }

        // Primary button (Turn on / Resume).
        if let Some((r, label)) = primary {
            // Outlined like the Windows "Turn on" button.
            let hot = self.hover == Hit::Primary;
            let fill = if hot { p.card_hover } else { p.card };
            c.round_rect(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32, pxf(4.0), fill, 1.0);
            c.round_rect_outline(r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32, pxf(4.0), pxf(1.5), p.text, 1.0);
            c.text(self.fonts.body, label, r, p.text, DT_C);
        }

        // Toast at the bottom ("Image saved").
        if let Some((text, until)) = &self.toast {
            if *until > ctx.now {
                let tw = c.text_width(self.fonts.small, text) + px(32.0);
                let tw = tw.min(w - px(32.0));
                let (tx, ty, th) = ((w - tw) / 2, h - px(52.0), px(32.0));
                c.round_rect(
                    tx as f32,
                    ty as f32,
                    tw as f32,
                    th as f32,
                    pxf(6.0),
                    if p.dark { 0x3A3A3A } else { 0x2B2B2B },
                    0.96,
                );
                c.text(self.fonts.small, text, (tx + px(12.0), ty, tw - px(24.0), th), 0xFFFFFF, DT_L);
            }
        }

        c.present(dc, 0, 0, w, h);
        self.painted_at = util::qpc_seconds();
        self.paint_ms = (self.painted_at - started) * 1000.0;
    }
}

/// Up to four preview lines, each cut to a sensible length, without copying huge texts.
fn preview_lines(text: &str, content: &crate::model::Content, per_line: usize) -> (String, i32) {
    let source: std::borrow::Cow<str> =
        if text.trim().is_empty() { content.preview_text().into() } else { std::borrow::Cow::Borrowed(text) };
    let mut out = String::new();
    let mut lines = 0;
    for raw in source.lines().map(|l| l.trim_end()).skip_while(|l| l.trim().is_empty()) {
        if lines >= 4 {
            break;
        }
        let line: String = raw.chars().take(per_line * 4).map(|c| if c == '\t' { ' ' } else { c }).collect();
        let wrapped = (line.chars().count().max(1) as f32 / per_line as f32).ceil() as i32;
        let take = wrapped.min(4 - lines).max(1);
        if !out.is_empty() {
            // DrawText only breaks lines on CR LF.
            out.push_str("\r\n");
        }
        if wrapped > take {
            out.extend(line.chars().take(per_line * take as usize));
        } else {
            out.push_str(&line);
        }
        lines += take;
    }
    (out, lines.max(1))
}

fn next_boundary(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

fn prev_boundary(s: &str, at: usize) -> usize {
    s[..at].chars().next_back().map_or(0, |c| at - c.len_utf8())
}

fn word_start(s: &str, at: usize) -> usize {
    let before = s[..at].trim_end();
    before.rfind(char::is_whitespace).map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_moves_over_multibyte_characters() {
        let s = "aä🌍b";
        assert_eq!(next_boundary(s, 0), 1);
        assert_eq!(next_boundary(s, 1), 3);
        assert_eq!(next_boundary(s, 3), 7);
        assert_eq!(prev_boundary(s, 7), 3);
        assert_eq!(prev_boundary(s, 0), 0);
        assert_eq!(word_start("hello big world", 15), 10);
        assert_eq!(word_start("hello", 5), 0);
    }

    #[test]
    fn previews_are_bounded() {
        let content = crate::model::Content::default();
        let long = "x".repeat(10_000);
        let (text, lines) = preview_lines(&long, &content, 40);
        assert_eq!(lines, 4);
        assert_eq!(text.chars().count(), 160);
        let (text, lines) = preview_lines("\n\n  \nfirst\nsecond", &content, 40);
        assert_eq!((text.as_str(), lines), ("first\r\nsecond", 2));
        let many = (0..20).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(preview_lines(&many, &content, 40).1, 4);
    }
}
