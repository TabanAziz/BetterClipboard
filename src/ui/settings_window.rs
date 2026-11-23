//! Settings and "New snippet" windows, built from standard Windows controls so keyboard
//! navigation, screen readers and IME work as users expect. Also the image save dialog.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Controls::SetWindowTheme;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::app;
use crate::settings::{Hotkey, Settings, Theme};
use crate::system;
use crate::util::{self, from_wide, wide};

thread_local! {
    static SETTINGS_UI: RefCell<Option<SettingsUi>> = const { RefCell::new(None) };
    static SNIPPET_UI: RefCell<Option<SnippetUi>> = const { RefCell::new(None) };
    /// Windows that need IsDialogMessage for Tab navigation.
    pub static DIALOGS: Cell<(isize, isize)> = const { Cell::new((0, 0)) };
}

const BG_BRUSH_DARK: u32 = 0x202020;

// Control ids.
const ID_HISTORY: i32 = 10;
const ID_PAUSED: i32 = 11;
const ID_HOTKEY: i32 = 12;
const ID_WIN_V: i32 = 13;
const ID_PASTE: i32 = 14;
const ID_SAVE_PINNED: i32 = 20;
const ID_KEEP: i32 = 21;
const ID_MAX_ITEMS: i32 = 22;
const ID_MAX_MB: i32 = 23;
const ID_RETENTION: i32 = 24;
const ID_GROUP: i32 = 25;
const ID_EXCLUDED: i32 = 30;
const ID_THEME: i32 = 40;
const ID_AUTOSTART: i32 = 50;
const ID_OPEN_WINDOWS: i32 = 51;
const ID_OPEN_PHONE: i32 = 52;
const ID_CLEAR: i32 = 53;
const ID_SAVE: i32 = 1; // IDOK
const ID_CANCEL: i32 = 2; // IDCANCEL

struct SettingsUi {
    window: HWND,
    font: HFONT,
    heading: HFONT,
    brush: HBRUSH,
    dark: bool,
    controls: Vec<(HWND, i32)>,
}

fn create_control(parent: HWND, class: &str, text: &str, style: u32, ex: u32, id: i32) -> HWND {
    unsafe {
        wm::CreateWindowExW(
            ex,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            wm::WS_CHILD | wm::WS_VISIBLE | style,
            0,
            0,
            10,
            10,
            parent,
            id as isize as _,
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    }
}

fn ui_font(dpi: u32, scale_size: f32, weight: i32) -> HFONT {
    unsafe {
        let mut metrics: wm::NONCLIENTMETRICSW = std::mem::zeroed();
        metrics.cbSize = std::mem::size_of::<wm::NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoForDpi(wm::SPI_GETNONCLIENTMETRICS, metrics.cbSize, &mut metrics as *mut _ as _, 0, dpi);
        let mut lf = metrics.lfMessageFont;
        lf.lfHeight = (lf.lfHeight as f32 * scale_size) as i32;
        lf.lfWeight = weight;
        CreateFontIndirectW(&lf)
    }
}

fn set_dark_title(window: HWND, dark: bool) {
    let value: i32 = dark as i32;
    unsafe { DwmSetWindowAttribute(window, 20, &value as *const i32 as _, 4) };
}

// MAKEINTRESOURCE(1) looks like a dangling pointer to clippy; it is a resource number.
#[allow(clippy::manual_dangling_ptr)]
fn register(class: &str, proc_: wm::WNDPROC) {
    unsafe {
        let name = wide(class);
        let mut definition: wm::WNDCLASSEXW = std::mem::zeroed();
        definition.cbSize = std::mem::size_of_val(&definition) as u32;
        definition.hInstance = GetModuleHandleW(std::ptr::null());
        definition.lpszClassName = name.as_ptr();
        definition.lpfnWndProc = proc_;
        definition.hCursor = wm::LoadCursorW(std::ptr::null_mut(), wm::IDC_ARROW);
        // MAKEINTRESOURCE(1): the application icon.
        let icon_id: usize = 1;
        definition.hIcon =
            wm::LoadImageW(definition.hInstance, icon_id as *const u16, wm::IMAGE_ICON, 0, 0, wm::LR_DEFAULTSIZE) as _;
        wm::RegisterClassExW(&definition);
    }
}

fn checked(window: HWND, id: i32) -> bool {
    unsafe { wm::SendMessageW(wm::GetDlgItem(window, id), wm::BM_GETCHECK, 0, 0) == 1 }
}

fn set_checked(window: HWND, id: i32, on: bool) {
    unsafe { wm::SendMessageW(wm::GetDlgItem(window, id), wm::BM_SETCHECK, on as usize, 0) };
}

fn text_of(window: HWND, id: i32) -> String {
    unsafe {
        let control = wm::GetDlgItem(window, id);
        let len = wm::GetWindowTextLengthW(control).max(0) as usize;
        let mut buffer = vec![0u16; len + 1];
        wm::GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32);
        from_wide(&buffer)
    }
}

fn set_text(window: HWND, id: i32, text: &str) {
    unsafe { wm::SetWindowTextW(wm::GetDlgItem(window, id), wide(text).as_ptr()) };
}

fn message_box(owner: HWND, text: &str, flags: u32) -> i32 {
    unsafe { wm::MessageBoxW(owner, wide(text).as_ptr(), wide("BetterClipboard").as_ptr(), flags) }
}

/// Layout rows: (kind, text, id). Kind: h = heading, c = checkbox, l = label, e = edit, b = button.
fn rows() -> Vec<(&'static str, &'static str, i32)> {
    vec![
        ("h", "Clipboard history", 0),
        ("c", "Record clipboard history", ID_HISTORY),
        ("c", "Pause recording for now", ID_PAUSED),
        ("e", "Shortcut to open the history", ID_HOTKEY),
        ("b", "Use Win+V...", ID_WIN_V),
        ("c", "Paste into the previous app when I choose an entry", ID_PASTE),
        ("h", "Storage", 0),
        ("c", "Save pinned entries (encrypted for your Windows account)", ID_SAVE_PINNED),
        ("c", "Also keep unpinned entries after a restart", ID_KEEP),
        ("c", "Group identical copies into one entry", ID_GROUP),
        ("e", "Unpinned entries to keep (Windows keeps 25)", ID_MAX_ITEMS),
        ("e", "Largest entry in MB (Windows uses 4)", ID_MAX_MB),
        ("e", "Remove unpinned entries after this many days (0 = never)", ID_RETENTION),
        ("h", "Privacy", 0),
        ("l", "Never record copies from these apps (one program file per line):", 0),
        ("m", "", ID_EXCLUDED),
        ("h", "Appearance and Windows", 0),
        ("t", "Theme", ID_THEME),
        ("c", "Start BetterClipboard when I sign in", ID_AUTOSTART),
        ("s", "", 0),
        ("w", "", 0),
    ]
}

pub fn open_settings() {
    let existing = SETTINGS_UI.with(|s| s.borrow().as_ref().map(|ui| ui.window));
    if let Some(window) = existing {
        unsafe {
            wm::ShowWindow(window, wm::SW_SHOWNORMAL);
            wm::SetForegroundWindow(window);
        }
        return;
    }
    let Some(settings) = app::with(|a| a.settings.clone()) else { return };
    let stats = app::with(|a| a.stats).unwrap_or_default();
    let class = format!("BetterClipboard.Settings{}", util::profile());
    register(&class, Some(settings_proc));
    let dark = crate::ui::theme::is_dark(settings.theme);
    let window = unsafe {
        wm::CreateWindowExW(
            0,
            wide(&class).as_ptr(),
            wide("BetterClipboard Settings").as_ptr(),
            wm::WS_OVERLAPPED | wm::WS_CAPTION | wm::WS_SYSMENU | wm::WS_MINIMIZEBOX | wm::WS_CLIPCHILDREN,
            wm::CW_USEDEFAULT,
            wm::CW_USEDEFAULT,
            100,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return;
    }
    set_dark_title(window, dark);
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let mut ui = SettingsUi {
        window,
        font: ui_font(dpi, 1.0, 400),
        heading: ui_font(dpi, 1.25, 600),
        brush: unsafe { CreateSolidBrush(crate::ui::gfx::cr(BG_BRUSH_DARK)) },
        dark,
        controls: Vec::new(),
    };

    const BS_AUTOCHECKBOX: u32 = 3;
    const BS_PUSHBUTTON: u32 = 0;
    const BS_DEFPUSHBUTTON: u32 = 1;
    const ES_AUTOHSCROLL: u32 = 0x80;
    const ES_MULTILINE: u32 = 4;
    const ES_AUTOVSCROLL: u32 = 0x40;
    const ES_WANTRETURN: u32 = 0x1000;
    const ES_NUMBER: u32 = 0x2000;
    const CBS_DROPDOWNLIST: u32 = 3;
    let tab = wm::WS_TABSTOP;
    let edge = wm::WS_EX_CLIENTEDGE;

    let windows_history = match system::windows_history_enabled() {
        Some(true) => "on",
        Some(false) => "off",
        None => "not set",
    };
    let cloud = match system::windows_cloud_sync_enabled() {
        Some(true) => "on",
        Some(false) => "off",
        None => "not set",
    };
    let status = format!(
        "Windows clipboard history is {windows_history}, sync across devices is {cloud}.\r\nTo get what you copy on your Android phone, turn on \"Cross-device copy and paste\" in Phone Link. Those entries show a phone icon and the Phone filter.\r\nThis session: {} recorded, {} skipped as private, {} from excluded apps, {} too large.",
        stats.captured, stats.skipped_private, stats.skipped_excluded, stats.skipped_large
    );

    for (kind, text, id) in rows() {
        let control = match kind {
            "h" => create_control(window, "STATIC", text, 0, 0, -1),
            "l" => create_control(window, "STATIC", text, 0, 0, -1),
            "c" => {
                let c = create_control(window, "BUTTON", text, BS_AUTOCHECKBOX | tab, 0, id);
                if dark {
                    unsafe { SetWindowTheme(c, wide("").as_ptr(), wide("").as_ptr()) };
                }
                c
            }
            "e" => {
                let label = create_control(window, "STATIC", text, 0, 0, -1);
                ui.controls.push((label, -2));
                let numeric = if id == ID_HOTKEY { 0 } else { ES_NUMBER };
                create_control(window, "EDIT", "", ES_AUTOHSCROLL | numeric | tab, edge, id)
            }
            "m" => create_control(
                window,
                "EDIT",
                "",
                ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | wm::WS_VSCROLL | tab,
                edge,
                id,
            ),
            "b" => create_control(window, "BUTTON", text, BS_PUSHBUTTON | tab, 0, id),
            "t" => {
                let label = create_control(window, "STATIC", text, 0, 0, -1);
                ui.controls.push((label, -2));
                let combo = create_control(window, "COMBOBOX", "", CBS_DROPDOWNLIST | tab | wm::WS_VSCROLL, 0, id);
                for option in ["Use my Windows setting", "Light", "Dark"] {
                    unsafe { wm::SendMessageW(combo, wm::CB_ADDSTRING, 0, wide(option).as_ptr() as LPARAM) };
                }
                combo
            }
            "s" => create_control(window, "STATIC", &status, 0, 0, -1),
            _ => {
                // Bottom row of buttons.
                for (id, text, style) in [
                    (ID_OPEN_WINDOWS, "Windows clipboard settings", BS_PUSHBUTTON),
                    (ID_OPEN_PHONE, "Open Phone Link", BS_PUSHBUTTON),
                    (ID_CLEAR, "Clear clipboard data", BS_PUSHBUTTON),
                    (ID_SAVE, "Save", BS_DEFPUSHBUTTON),
                    (ID_CANCEL, "Cancel", BS_PUSHBUTTON),
                ] {
                    let b = create_control(window, "BUTTON", text, style | tab, 0, id);
                    ui.controls.push((b, -3));
                }
                continue;
            }
        };
        let tag = match kind {
            "h" => -1,
            "m" => -4,
            "s" => -5,
            "b" => -6,
            _ => id,
        };
        ui.controls.push((control, tag));
    }
    for (control, tag) in &ui.controls {
        let font = if *tag == -1 { ui.heading } else { ui.font };
        unsafe { wm::SendMessageW(*control, wm::WM_SETFONT, font as usize, 1) };
    }
    if dark {
        // Windows' own dark styles for push buttons, edits and the combo box.
        unsafe {
            for id in [ID_WIN_V, ID_OPEN_WINDOWS, ID_OPEN_PHONE, ID_CLEAR, ID_SAVE, ID_CANCEL, ID_EXCLUDED] {
                SetWindowTheme(wm::GetDlgItem(window, id), wide("DarkMode_Explorer").as_ptr(), std::ptr::null());
            }
            SetWindowTheme(wm::GetDlgItem(window, ID_THEME), wide("DarkMode_CFD").as_ptr(), std::ptr::null());
        }
    }
    layout_settings(&ui, dpi);

    set_checked(window, ID_HISTORY, settings.history_enabled);
    set_checked(window, ID_PAUSED, settings.paused);
    set_text(window, ID_HOTKEY, &settings.hotkey.label());
    set_checked(window, ID_PASTE, settings.paste_on_select);
    set_checked(window, ID_SAVE_PINNED, settings.save_pinned);
    set_checked(window, ID_KEEP, settings.keep_history);
    set_checked(window, ID_GROUP, settings.group_duplicates);
    set_text(window, ID_MAX_ITEMS, &settings.max_items.to_string());
    set_text(window, ID_MAX_MB, &settings.max_item_mb.to_string());
    set_text(window, ID_RETENTION, &settings.retention_days.to_string());
    set_text(window, ID_EXCLUDED, &settings.excluded_apps.join("\r\n"));
    let theme_index = match settings.theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    };
    unsafe { wm::SendMessageW(wm::GetDlgItem(window, ID_THEME), wm::CB_SETCURSEL, theme_index, 0) };
    set_checked(window, ID_AUTOSTART, system::autostart_enabled());
    if system::history_blocked_by_policy() {
        set_text(window, ID_HISTORY, "Record clipboard history (turned off by your organization)");
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(wm::GetDlgItem(window, ID_HISTORY), 0) };
    }
    if system::windows_win_v_disabled() {
        set_text(window, ID_WIN_V, "Give Win+V back to Windows...");
    }

    SETTINGS_UI.with(|s| *s.borrow_mut() = Some(ui));
    DIALOGS.with(|d| d.set((window as isize, d.get().1)));
    if let Some(()) = app::with(|a| a.settings_window = window) {}
    unsafe {
        wm::ShowWindow(window, wm::SW_SHOWNORMAL);
        wm::SetForegroundWindow(window);
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(wm::GetDlgItem(window, ID_HISTORY));
    }
}

fn layout_settings(ui: &SettingsUi, dpi: u32) {
    let px = |dip: i32| dip * dpi as i32 / 96;
    let width = px(560);
    let margin = px(24);
    let inner = width - 2 * margin;
    let mut y = px(16);
    let mut bottom_buttons = Vec::new();
    let mut i = 0;
    while i < ui.controls.len() {
        let (control, tag) = ui.controls[i];
        let place = |c: HWND, x: i32, y: i32, w: i32, h: i32| unsafe {
            wm::SetWindowPos(c, std::ptr::null_mut(), x, y, w, h, wm::SWP_NOZORDER | wm::SWP_NOACTIVATE);
        };
        match tag {
            -1 => {
                y += px(10);
                place(control, margin, y, inner, px(26));
                y += px(32);
            }
            -2 => {
                // Label followed by its edit or combo box on the same row.
                let (field, field_tag) = ui.controls[i + 1];
                let field_w = if field_tag == ID_HOTKEY {
                    px(150)
                } else if field_tag == ID_THEME {
                    px(200)
                } else {
                    px(70)
                };
                // The shortcut row also holds the "Use Win+V" button.
                let extra = if field_tag == ID_HOTKEY { px(150) } else { 0 };
                place(control, margin, y + px(3), inner - field_w - extra - px(12), px(22));
                place(
                    field,
                    margin + inner - field_w,
                    y,
                    field_w,
                    if field_tag == ID_THEME { px(200) } else { px(26) },
                );
                y += px(34);
                i += 1;
            }
            -3 => bottom_buttons.push(control),
            -4 => {
                place(control, margin, y, inner, px(80));
                y += px(88);
            }
            -5 => {
                y += px(6);
                place(control, margin, y, inner, px(84));
                y += px(90);
            }
            -6 => {
                place(control, margin + inner - px(150) - px(8) - px(142), y - px(34) - px(1), px(142), px(28));
            }
            _ => {
                place(control, margin, y, inner, px(24));
                y += px(30);
            }
        }
        i += 1;
    }
    // Buttons: three on the left, Save/Cancel on the right.
    let h = px(30);
    let mut x = margin;
    for (n, control) in bottom_buttons.iter().enumerate() {
        let w = if n < 3 { px(if n == 0 { 180 } else { 120 }) } else { px(84) };
        if n == 3 {
            x = width - margin - 2 * px(84) - px(8);
        }
        if n < 3 {
            // Left buttons in a row above Save/Cancel to keep labels readable.
            unsafe {
                wm::SetWindowPos(*control, std::ptr::null_mut(), x, y, w, h, wm::SWP_NOZORDER | wm::SWP_NOACTIVATE)
            };
            x += w + px(8);
            if n == 2 {
                y += h + px(12);
                x = width - margin - 2 * px(84) - px(8);
            }
            continue;
        }
        unsafe { wm::SetWindowPos(*control, std::ptr::null_mut(), x, y, w, h, wm::SWP_NOZORDER | wm::SWP_NOACTIVATE) };
        x += w + px(8);
    }
    y += h + px(20);
    let mut frame = RECT { left: 0, top: 0, right: width, bottom: y };
    unsafe {
        let style = wm::GetWindowLongW(ui.window, wm::GWL_STYLE) as u32;
        windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut frame, style, 0, 0, dpi);
        wm::SetWindowPos(
            ui.window,
            std::ptr::null_mut(),
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            wm::SWP_NOMOVE | wm::SWP_NOZORDER | wm::SWP_NOACTIVATE,
        );
    }
}

fn read_settings(window: HWND, base: &Settings) -> Result<Settings, String> {
    let mut s = base.clone();
    s.history_enabled = checked(window, ID_HISTORY) || (system::history_blocked_by_policy() && base.history_enabled);
    s.paused = checked(window, ID_PAUSED);
    s.hotkey = Hotkey::parse(&text_of(window, ID_HOTKEY))
        .ok_or("The shortcut was not understood. Write it like Win+Alt+V or Ctrl+Shift+Space.")?;
    s.paste_on_select = checked(window, ID_PASTE);
    s.save_pinned = checked(window, ID_SAVE_PINNED);
    s.keep_history = checked(window, ID_KEEP);
    s.group_duplicates = checked(window, ID_GROUP);
    let number = |id: i32, what: &str, min: u32, max: u32| -> Result<u32, String> {
        text_of(window, id)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|v| (min..=max).contains(v))
            .ok_or(format!("{what} must be a number from {min} to {max}."))
    };
    s.max_items = number(ID_MAX_ITEMS, "Entries to keep", 1, crate::settings::MAX_ITEMS_LIMIT)?;
    s.max_item_mb = number(ID_MAX_MB, "The largest entry size", 1, crate::settings::MAX_ITEM_MB_LIMIT)?;
    s.retention_days = number(ID_RETENTION, "Days to keep entries", 0, 3650)?;
    s.excluded_apps = text_of(window, ID_EXCLUDED)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.rsplit('\\').next().unwrap_or(l).to_string())
        .collect();
    let theme = unsafe { wm::SendMessageW(wm::GetDlgItem(window, ID_THEME), wm::CB_GETCURSEL, 0, 0) };
    s.theme = match theme {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::System,
    };
    Ok(s)
}

fn close_settings() {
    if let Some(ui) = SETTINGS_UI.with(|s| s.borrow_mut().take()) {
        unsafe {
            wm::DestroyWindow(ui.window);
            DeleteObject(ui.font);
            DeleteObject(ui.heading);
            DeleteObject(ui.brush);
        }
    }
    DIALOGS.with(|d| d.set((0, d.get().1)));
    app::with(|a| a.settings_window = std::ptr::null_mut());
}

fn on_settings_command(window: HWND, id: i32) {
    match id {
        ID_SAVE => {
            let Some(base) = app::with(|a| a.settings.clone()) else { return };
            match read_settings(window, &base) {
                Ok(next) => {
                    if let Err(error) = system::set_autostart(checked(window, ID_AUTOSTART)) {
                        message_box(window, &error, wm::MB_ICONWARNING);
                    }
                    let notice = app::with(|a| {
                        a.apply_settings(next);
                        a.notice.clone()
                    })
                    .flatten();
                    if let Some(notice) = notice {
                        message_box(window, &notice, wm::MB_ICONINFORMATION);
                    }
                    close_settings();
                }
                Err(error) => {
                    message_box(window, &error, wm::MB_ICONWARNING);
                }
            }
        }
        ID_CANCEL => close_settings(),
        ID_OPEN_WINDOWS => {
            system::open("ms-settings:clipboard");
        }
        ID_OPEN_PHONE => {
            if !system::open("ms-phone:") {
                system::open("ms-settings:mobile-devices");
            }
        }
        ID_CLEAR => {
            let answer = message_box(
                window,
                "Clear clipboard data?\n\nThis removes every entry that is not pinned, from memory and from disk, and empties the current clipboard.",
                wm::MB_YESNO | wm::MB_ICONQUESTION,
            );
            if answer == wm::IDYES {
                app::with(|a| a.clear_all(true));
            }
        }
        ID_WIN_V => {
            if system::windows_win_v_disabled() {
                let answer = message_box(
                    window,
                    "Give Win+V back to Windows?\n\nThe Windows clipboard panel opens with Win+V again after you sign out and back in. BetterClipboard goes back to Win+Alt+V.",
                    wm::MB_YESNO | wm::MB_ICONQUESTION,
                );
                if answer == wm::IDYES {
                    match system::set_windows_win_v_disabled(false) {
                        Ok(()) => {
                            set_text(window, ID_HOTKEY, &Hotkey::DEFAULT.label());
                            set_text(window, ID_WIN_V, "Use Win+V...");
                        }
                        Err(e) => {
                            message_box(window, &e, wm::MB_ICONWARNING);
                        }
                    }
                }
            } else {
                let answer = message_box(
                    window,
                    "Use Win+V for BetterClipboard?\n\nWindows reserves Win+V for its own clipboard panel. BetterClipboard can switch that shortcut off for your account through Explorer's documented DisabledHotkeys setting. It takes effect after you sign out and back in, and you can undo it here or by uninstalling.\n\nUntil then BetterClipboard keeps its current shortcut.",
                    wm::MB_YESNO | wm::MB_ICONQUESTION,
                );
                if answer == wm::IDYES {
                    match system::set_windows_win_v_disabled(true) {
                        Ok(()) => {
                            set_text(window, ID_HOTKEY, &Hotkey::WIN_V.label());
                            set_text(window, ID_WIN_V, "Give Win+V back to Windows...");
                        }
                        Err(e) => {
                            message_box(window, &e, wm::MB_ICONWARNING);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let code = ((wparam >> 16) & 0xFFFF) as u32;
            if code == 0 || id == ID_SAVE || id == ID_CANCEL {
                on_settings_command(hwnd, id);
            }
            0
        }
        wm::WM_CTLCOLORSTATIC | wm::WM_CTLCOLOREDIT | wm::WM_CTLCOLORBTN | wm::WM_CTLCOLORLISTBOX => {
            let dark = SETTINGS_UI.with(|s| s.borrow().as_ref().filter(|ui| ui.dark).map(|ui| ui.brush));
            match dark {
                Some(brush) => {
                    let dc = wparam as HDC;
                    SetTextColor(dc, crate::ui::gfx::cr(0xFFFFFF));
                    SetBkColor(dc, crate::ui::gfx::cr(BG_BRUSH_DARK));
                    brush as LRESULT
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_ERASEBKGND => {
            let brush = SETTINGS_UI.with(|s| s.borrow().as_ref().filter(|ui| ui.dark).map(|ui| ui.brush));
            match brush {
                Some(brush) => {
                    let mut r: RECT = std::mem::zeroed();
                    wm::GetClientRect(hwnd, &mut r);
                    FillRect(wparam as HDC, &r, brush);
                    1
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            let suggested = *(lparam as *const RECT);
            wm::SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                wm::SWP_NOZORDER | wm::SWP_NOACTIVATE,
            );
            SETTINGS_UI.with(|s| {
                if let Some(ui) = s.borrow_mut().as_mut() {
                    DeleteObject(ui.font);
                    DeleteObject(ui.heading);
                    ui.font = ui_font(dpi, 1.0, 400);
                    ui.heading = ui_font(dpi, 1.25, 600);
                    for (control, tag) in &ui.controls {
                        let font = if *tag == -1 { ui.heading } else { ui.font };
                        wm::SendMessageW(*control, wm::WM_SETFONT, font as usize, 1);
                    }
                    layout_settings(ui, dpi);
                }
            });
            0
        }
        wm::WM_CLOSE => {
            close_settings();
            0
        }
        _ => {
            if msg == wm::WM_CTLCOLORDLG {
                return wm::DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            wm::DefWindowProcW(hwnd, msg, wparam, lparam)
        }
    }
}

// ---------------------------------------------------------------- snippet window

struct SnippetUi {
    window: HWND,
    font: HFONT,
    brush: HBRUSH,
    dark: bool,
}

const ID_SNIPPET_TEXT: i32 = 100;

pub fn open_snippet() {
    let existing = SNIPPET_UI.with(|s| s.borrow().as_ref().map(|ui| ui.window));
    if let Some(window) = existing {
        unsafe { wm::SetForegroundWindow(window) };
        return;
    }
    let theme = app::with(|a| a.settings.theme).unwrap_or(Theme::System);
    let dark = crate::ui::theme::is_dark(theme);
    let class = format!("BetterClipboard.Snippet{}", util::profile());
    register(&class, Some(snippet_proc));
    let window = unsafe {
        wm::CreateWindowExW(
            0,
            wide(&class).as_ptr(),
            wide("New snippet").as_ptr(),
            wm::WS_OVERLAPPED | wm::WS_CAPTION | wm::WS_SYSMENU | wm::WS_CLIPCHILDREN,
            wm::CW_USEDEFAULT,
            wm::CW_USEDEFAULT,
            100,
            100,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return;
    }
    set_dark_title(window, dark);
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    let px = |dip: i32| dip * dpi as i32 / 96;
    let font = ui_font(dpi, 1.0, 400);
    let label = create_control(window, "STATIC", "Text to keep as a pinned snippet:", 0, 0, -1);
    let edit = create_control(
        window,
        "EDIT",
        "",
        4 | 0x40 | 0x1000 | wm::WS_VSCROLL | wm::WS_TABSTOP,
        wm::WS_EX_CLIENTEDGE,
        ID_SNIPPET_TEXT,
    );
    let save = create_control(window, "BUTTON", "Save snippet", 1 | wm::WS_TABSTOP, 0, ID_SAVE);
    let cancel = create_control(window, "BUTTON", "Cancel", wm::WS_TABSTOP, 0, ID_CANCEL);
    let (w, margin) = (px(460), px(20));
    unsafe {
        for (c, x, y, cw, ch) in [
            (label, margin, px(16), w - 2 * margin, px(22)),
            (edit, margin, px(42), w - 2 * margin, px(160)),
            (save, w - margin - px(220), px(214), px(120), px(30)),
            (cancel, w - margin - px(92), px(214), px(92), px(30)),
        ] {
            wm::SendMessageW(c, wm::WM_SETFONT, font as usize, 1);
            wm::SetWindowPos(c, std::ptr::null_mut(), x, y, cw, ch, wm::SWP_NOZORDER);
        }
        let mut frame = RECT { left: 0, top: 0, right: w, bottom: px(262) };
        windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut frame,
            wm::GetWindowLongW(window, wm::GWL_STYLE) as u32,
            0,
            0,
            dpi,
        );
        wm::SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            frame.right - frame.left,
            frame.bottom - frame.top,
            wm::SWP_NOMOVE | wm::SWP_NOZORDER,
        );
    }
    let brush = unsafe { CreateSolidBrush(crate::ui::gfx::cr(BG_BRUSH_DARK)) };
    SNIPPET_UI.with(|s| *s.borrow_mut() = Some(SnippetUi { window, font, brush, dark }));
    DIALOGS.with(|d| d.set((d.get().0, window as isize)));
    unsafe {
        wm::ShowWindow(window, wm::SW_SHOWNORMAL);
        wm::SetForegroundWindow(window);
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(edit);
    }
}

fn close_snippet() {
    if let Some(ui) = SNIPPET_UI.with(|s| s.borrow_mut().take()) {
        unsafe {
            wm::DestroyWindow(ui.window);
            DeleteObject(ui.font);
            DeleteObject(ui.brush);
        }
    }
    DIALOGS.with(|d| d.set((d.get().0, 0)));
}

unsafe extern "system" fn snippet_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_COMMAND => {
            match (wparam & 0xFFFF) as i32 {
                ID_SAVE => {
                    let text = text_of(hwnd, ID_SNIPPET_TEXT);
                    if text.trim().is_empty() {
                        message_box(hwnd, "Type the text you want to keep first.", wm::MB_ICONINFORMATION);
                    } else {
                        app::with(|a| a.add_snippet(text));
                        close_snippet();
                    }
                }
                ID_CANCEL => close_snippet(),
                _ => {}
            }
            0
        }
        wm::WM_CTLCOLORSTATIC | wm::WM_CTLCOLOREDIT | wm::WM_CTLCOLORBTN => {
            let dark = SNIPPET_UI.with(|s| s.borrow().as_ref().filter(|ui| ui.dark).map(|ui| ui.brush));
            match dark {
                Some(brush) => {
                    let dc = wparam as HDC;
                    SetTextColor(dc, crate::ui::gfx::cr(0xFFFFFF));
                    SetBkColor(dc, crate::ui::gfx::cr(BG_BRUSH_DARK));
                    brush as LRESULT
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_ERASEBKGND => {
            let brush = SNIPPET_UI.with(|s| s.borrow().as_ref().filter(|ui| ui.dark).map(|ui| ui.brush));
            match brush {
                Some(brush) => {
                    let mut r: RECT = std::mem::zeroed();
                    wm::GetClientRect(hwnd, &mut r);
                    FillRect(wparam as HDC, &r, brush);
                    1
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_CLOSE => {
            close_snippet();
            0
        }
        _ => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------- helpers

/// Local time as "2026-11-23 134512", for default file names.
pub fn local_timestamp(unix_ms: i64) -> String {
    let ticks = (unix_ms.max(0) as u64) * 10_000 + 116_444_736_000_000_000;
    let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&file_time, &mut utc) == 0
            || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0
        {
            return unix_ms.to_string();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}{:02}{:02}",
            local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
        )
    }
}

/// The standard Save As dialog for an image. Returns the chosen path with an extension.
pub fn save_file_dialog(owner: HWND, suggested: &str) -> Option<PathBuf> {
    let mut file = [0u16; 1024];
    for (i, unit) in suggested.encode_utf16().take(1000).enumerate() {
        file[i] = unit;
    }
    let filter: Vec<u16> = "PNG image (*.png)\0*.png\0Bitmap image (*.bmp)\0*.bmp\0\0".encode_utf16().collect();
    let pictures = std::env::var("USERPROFILE").map(|p| format!("{p}\\Pictures")).unwrap_or_default();
    let initial = wide(&pictures);
    let extension = wide("png");
    let title = wide("Save image");
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrInitialDir = initial.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR;
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let mut path = PathBuf::from(from_wide(&file));
    let wanted = if dialog.nFilterIndex == 2 { "bmp" } else { "png" };
    let has = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    if has.as_deref() != Some("png") && has.as_deref() != Some("bmp") {
        path.set_extension(wanted);
    }
    Some(path)
}
