//! Pasting into the window that was active before the panel opened.
//!
//! The entry is put on the clipboard first; only then focus goes back to the target and a
//! normal Ctrl+V is sent, so the target reads the data through its own paste path.
//! Windows blocks input into apps with a higher integrity level (UIPI). Those cases are
//! detected up front and reported instead of failing silently.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, IsWindow, SetForegroundWindow, GA_ROOT,
};

use crate::system;

#[derive(Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The target window is gone.
    TargetClosed,
    /// The target runs with higher rights (for example as administrator).
    Elevated,
}

pub fn check_target(target: HWND) -> Result<(), Blocked> {
    if target.is_null() || unsafe { IsWindow(target) } == 0 {
        return Err(Blocked::TargetClosed);
    }
    let own = system::own_integrity().unwrap_or(0x2000);
    match system::window_integrity(target) {
        Some(level) if level <= own => Ok(()),
        _ => Err(Blocked::Elevated),
    }
}

fn root(window: HWND) -> HWND {
    unsafe { GetAncestor(window, GA_ROOT) }
}

/// Asks Windows to activate the target. Returns true once it is in the foreground.
pub fn focus(target: HWND) -> bool {
    unsafe {
        if root(GetForegroundWindow()) == root(target) {
            return true;
        }
        SetForegroundWindow(target);
        root(GetForegroundWindow()) == root(target)
    }
}

fn key(vk: u16, up: bool) -> INPUT {
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki =
        KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 };
    input
}

/// Opens the Windows emoji panel (Win+.) in the focused app. It hosts the emoji, GIF,
/// kaomoji and symbol pages that the Windows clipboard panel shows as tabs.
pub fn open_emoji_panel() -> bool {
    let inputs = [key(VK_LWIN, false), key(VK_OEM_PERIOD, false), key(VK_OEM_PERIOD, true), key(VK_LWIN, true)];
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

/// Types text into the focused window as Unicode keyboard input (no clipboard involved).
pub fn type_text(text: &str) -> bool {
    let mut inputs = Vec::new();
    for unit in text.encode_utf16() {
        for up in [false, true] {
            let mut input: INPUT = unsafe { std::mem::zeroed() };
            input.r#type = INPUT_KEYBOARD;
            input.Anonymous.ki = KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            };
            inputs.push(input);
        }
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

/// Sends Ctrl+V. Modifiers still held from the hotkey are released first; a harmless
/// unassigned key keeps a released Windows key from opening Start.
pub fn send_ctrl_v() -> bool {
    let mut inputs = Vec::new();
    let held = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0;
    let win_held = held(VK_LWIN) || held(VK_RWIN);
    if win_held {
        inputs.push(key(0xE8, false));
        inputs.push(key(0xE8, true));
    }
    for vk in [VK_LWIN, VK_RWIN, VK_LMENU, VK_RMENU, VK_LSHIFT, VK_RSHIFT, VK_RCONTROL] {
        if held(vk) {
            inputs.push(key(vk, true));
        }
    }
    let ctrl_held = held(VK_LCONTROL);
    if !ctrl_held {
        inputs.push(key(VK_LCONTROL, false));
    }
    inputs.push(key(b'V' as u16, false));
    inputs.push(key(b'V' as u16, true));
    if !ctrl_held {
        inputs.push(key(VK_LCONTROL, true));
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}
