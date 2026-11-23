//! Notification area icon, its menu and balloon notifications.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

pub const WM_TRAY: u32 = wm::WM_APP + 10;
const ICON_ID: u32 = 1;

pub mod menu {
    pub const OPEN: usize = 100;
    pub const PAUSE: usize = 101;
    pub const HISTORY: usize = 102;
    pub const SNIPPET: usize = 103;
    pub const SETTINGS: usize = 104;
    pub const CLEAR: usize = 105;
    pub const PHONE_LINK: usize = 106;
    pub const WINDOWS_SETTINGS: usize = 107;
    pub const EXIT: usize = 108;
}

pub struct Tray {
    window: HWND,
    added: bool,
}

fn copy_into(target: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(target.len() - 1).collect();
    target[..units.len()].copy_from_slice(&units);
    target[units.len()] = 0;
}

fn icon(resource: usize) -> wm::HICON {
    unsafe {
        let size = wm::GetSystemMetrics(wm::SM_CXSMICON);
        wm::LoadImageW(GetModuleHandleW(std::ptr::null()), resource as *const u16, wm::IMAGE_ICON, size, size, 0) as _
    }
}

impl Tray {
    pub fn new(window: HWND) -> Tray {
        Tray { window, added: false }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.window;
        data.uID = ICON_ID;
        data
    }

    /// Adds the icon (again after Explorer restarts).
    pub fn add(&mut self, paused: bool, tip: &str) {
        let mut data = self.data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon(if paused { 2 } else { 1 });
        copy_into(&mut data.szTip, tip);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data);
            self.added = Shell_NotifyIconW(NIM_ADD, &data) != 0;
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
    }

    pub fn update(&self, paused: bool, tip: &str) {
        let mut data = self.data();
        data.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.hIcon = icon(if paused { 2 } else { 1 });
        copy_into(&mut data.szTip, tip);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    pub fn notify(&self, title: &str, text: &str) {
        let mut data = self.data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
        data.hBalloonIcon = icon(1);
        copy_into(&mut data.szInfoTitle, title);
        copy_into(&mut data.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    pub fn remove(&mut self) {
        if self.added {
            let data = self.data();
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            self.added = false;
        }
    }
}

pub struct MenuState<'a> {
    pub hotkey: &'a str,
    pub paused: bool,
    pub history_enabled: bool,
}

/// Shows the tray menu at the given screen point and returns the chosen command.
pub fn show_menu(window: HWND, x: i32, y: i32, state: &MenuState) -> usize {
    use crate::util::wide;
    unsafe {
        let popup = wm::CreatePopupMenu();
        let add = |id: usize, text: &str, checked: bool, enabled: bool| {
            let mut flags = wm::MF_STRING;
            if checked {
                flags |= wm::MF_CHECKED;
            }
            if !enabled {
                flags |= wm::MF_GRAYED;
            }
            wm::AppendMenuW(popup, flags, id, wide(text).as_ptr());
        };
        let separator = || {
            wm::AppendMenuW(popup, wm::MF_SEPARATOR, 0, std::ptr::null());
        };
        add(menu::OPEN, &format!("Open clipboard history\t{}", state.hotkey), false, true);
        add(menu::SNIPPET, "New snippet...", false, true);
        separator();
        add(menu::HISTORY, "Clipboard history", state.history_enabled, true);
        add(menu::PAUSE, "Pause recording", state.paused, state.history_enabled);
        add(menu::CLEAR, "Clear history (keeps pinned)", false, true);
        separator();
        add(menu::PHONE_LINK, "Open Phone Link", false, true);
        add(menu::WINDOWS_SETTINGS, "Windows clipboard settings", false, true);
        add(menu::SETTINGS, "Settings", false, true);
        separator();
        add(menu::EXIT, "Exit", false, true);
        wm::SetMenuDefaultItem(popup, menu::OPEN as u32, 0);
        // The owner must be in the foreground, or the menu does not close when clicking elsewhere.
        wm::SetForegroundWindow(window);
        let chosen = wm::TrackPopupMenuEx(
            popup,
            wm::TPM_RETURNCMD | wm::TPM_RIGHTBUTTON | wm::TPM_BOTTOMALIGN,
            x,
            y,
            window,
            std::ptr::null(),
        );
        wm::PostMessageW(window, wm::WM_NULL, 0, 0);
        wm::DestroyMenu(popup);
        chosen as usize
    }
}
