//! Small standard dialogs: "New snippet" and the image Save As dialog, plus helpers for
//! windows built from standard controls.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use super::gfx::cr;
use super::theme::{self, Palette};
use crate::app;
use crate::settings::Theme;
use crate::util::{self, from_wide, wide};

thread_local! {
    static SNIPPET_UI: RefCell<Option<SnippetUi>> = const { RefCell::new(None) };
    /// The snippet window, which needs IsDialogMessage for Tab navigation.
    pub static SNIPPET_WINDOW: Cell<isize> = const { Cell::new(0) };
}

const ID_SAVE: i32 = 1; // IDOK
const ID_CANCEL: i32 = 2; // IDCANCEL

pub fn create_control(parent: HWND, class: &str, text: &str, style: u32, ex: u32, id: i32) -> HWND {
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

pub fn ui_font(dpi: u32, scale_size: f32, weight: i32) -> HFONT {
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

pub fn set_theme_title(window: HWND, palette: Palette) {
    let dark = palette.dark as i32;
    let caption = if palette.motifs { cr(palette.header) } else { DWMWA_COLOR_DEFAULT };
    let text = if palette.motifs { cr(palette.text) } else { DWMWA_COLOR_DEFAULT };
    unsafe {
        DwmSetWindowAttribute(window, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, (&dark as *const i32).cast(), 4);
        DwmSetWindowAttribute(window, DWMWA_CAPTION_COLOR as u32, (&caption as *const u32).cast(), 4);
        DwmSetWindowAttribute(window, DWMWA_TEXT_COLOR as u32, (&text as *const u32).cast(), 4);
    }
}

// MAKEINTRESOURCE(1) looks like a dangling pointer to clippy; it is a resource number.
#[allow(clippy::manual_dangling_ptr)]
pub fn register(class: &str, proc_: wm::WNDPROC) {
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

pub fn text_of(window: HWND, id: i32) -> String {
    unsafe {
        let control = wm::GetDlgItem(window, id);
        let len = wm::GetWindowTextLengthW(control).max(0) as usize;
        let mut buffer = vec![0u16; len + 1];
        wm::GetWindowTextW(control, buffer.as_mut_ptr(), buffer.len() as i32);
        from_wide(&buffer)
    }
}

pub fn message_box(owner: HWND, text: &str, flags: u32) -> i32 {
    unsafe { wm::MessageBoxW(owner, wide(text).as_ptr(), wide("BetterClipboard").as_ptr(), flags) }
}

// ---------------------------------------------------------------- snippet window

struct SnippetUi {
    window: HWND,
    font: HFONT,
    brush: HBRUSH,
    field_brush: HBRUSH,
    palette: Option<Palette>,
}

impl SnippetUi {
    fn set_theme(&mut self, preference: Theme) {
        let mut palette = theme::palette(preference);
        set_theme_title(self.window, palette);
        if !preference.is_custom() && !palette.contrast {
            // Keep the standard dialog in the original Windows light or dark style.
            palette.field = palette.background;
        }
        self.palette = (preference.is_custom() || palette.dark || palette.contrast).then_some(palette);
        unsafe {
            DeleteObject(self.brush);
            DeleteObject(self.field_brush);
            self.brush = CreateSolidBrush(cr(palette.background));
            self.field_brush = CreateSolidBrush(cr(palette.field));
            RedrawWindow(
                self.window,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }
}

const ID_SNIPPET_TEXT: i32 = 100;

pub fn open_snippet() {
    let existing = SNIPPET_UI.with(|s| s.borrow().as_ref().map(|ui| ui.window));
    if let Some(window) = existing {
        unsafe { wm::SetForegroundWindow(window) };
        return;
    }
    let theme = app::with(|a| a.settings.theme).unwrap_or(Theme::System);
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
    let mut ui =
        SnippetUi { window, font, brush: std::ptr::null_mut(), field_brush: std::ptr::null_mut(), palette: None };
    ui.set_theme(theme);
    SNIPPET_UI.with(|s| *s.borrow_mut() = Some(ui));
    SNIPPET_WINDOW.with(|d| d.set(window as isize));
    unsafe {
        wm::ShowWindow(window, wm::SW_SHOWNORMAL);
        wm::SetForegroundWindow(window);
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(edit);
    }
}

pub fn refresh_snippet_theme(theme: Theme) {
    SNIPPET_UI.with(|s| {
        if let Some(ui) = s.borrow_mut().as_mut() {
            ui.set_theme(theme);
        }
    });
}

fn close_snippet() {
    if let Some(ui) = SNIPPET_UI.with(|s| s.borrow_mut().take()) {
        unsafe {
            wm::DestroyWindow(ui.window);
            DeleteObject(ui.font);
            DeleteObject(ui.brush);
            DeleteObject(ui.field_brush);
        }
    }
    SNIPPET_WINDOW.with(|d| d.set(0));
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
            let colors = SNIPPET_UI.with(|s| {
                let ui = s.try_borrow().ok()?;
                let ui = ui.as_ref()?;
                let palette = ui.palette?;
                let (brush, back) = if msg == wm::WM_CTLCOLOREDIT {
                    (ui.field_brush, palette.field)
                } else {
                    (ui.brush, palette.background)
                };
                Some((brush, back, palette.text))
            });
            match colors {
                Some((brush, back, text)) => {
                    let dc = wparam as HDC;
                    SetTextColor(dc, cr(text));
                    SetBkColor(dc, cr(back));
                    brush as LRESULT
                }
                None => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        wm::WM_ERASEBKGND => {
            let brush = SNIPPET_UI.with(|s| {
                let ui = s.try_borrow().ok()?;
                let ui = ui.as_ref()?;
                ui.palette.map(|_| ui.brush)
            });
            let brush = brush.unwrap_or_else(|| GetSysColorBrush(COLOR_BTNFACE));
            let mut r: RECT = std::mem::zeroed();
            wm::GetClientRect(hwnd, &mut r);
            FillRect(wparam as HDC, &r, brush);
            1
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
