//! The setup dialog shown when the program runs as `BetterClipboard-Setup-<version>.exe`
//! (or with `--setup`). It is a standard Windows task dialog: install for the current
//! user, choose whether Win+V opens BetterClipboard, and whether it starts at sign-in.

use windows_sys::Win32::Foundation::{BOOL, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::settings::{Hotkey, Settings};
use crate::util::wide;

const RADIO_WIN_V: i32 = 101;
const RADIO_KEEP: i32 = 102;

pub struct Choice {
    pub win_v: bool,
    pub autostart: bool,
}

/// True when the running file is named like a setup program.
pub fn launched_as_setup() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|stem| stem.contains("setup"))
}

fn task_dialog(
    title: &str,
    instruction: &str,
    content: &str,
    buttons: &[(i32, &str)],
    radios: &[(i32, &str)],
    verification: Option<&str>,
    common: i32,
) -> Option<(i32, i32, bool)> {
    let title = wide(title);
    let instruction = wide(instruction);
    let content = wide(content);
    let button_text: Vec<Vec<u16>> = buttons.iter().map(|(_, t)| wide(t)).collect();
    let button_list: Vec<TASKDIALOG_BUTTON> = buttons
        .iter()
        .zip(&button_text)
        .map(|((id, _), t)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: t.as_ptr() })
        .collect();
    let radio_text: Vec<Vec<u16>> = radios.iter().map(|(_, t)| wide(t)).collect();
    let radio_list: Vec<TASKDIALOG_BUTTON> = radios
        .iter()
        .zip(&radio_text)
        .map(|((id, _), t)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: t.as_ptr() })
        .collect();
    let verification = verification.map(wide);
    let icon = crate::tray::app_icon(unsafe { wm::GetSystemMetrics(wm::SM_CXICON) } * 3 / 2);
    unsafe {
        let mut config: TASKDIALOGCONFIG = std::mem::zeroed();
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.hInstance = GetModuleHandleW(std::ptr::null());
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_USE_HICON_MAIN | TDF_SIZE_TO_CONTENT;
        if verification.is_some() {
            config.dwFlags |= TDF_VERIFICATION_FLAG_CHECKED;
            config.pszVerificationText = verification.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        }
        config.dwCommonButtons = common;
        config.pszWindowTitle = title.as_ptr();
        config.Anonymous1.hMainIcon = icon;
        config.pszMainInstruction = instruction.as_ptr();
        config.pszContent = content.as_ptr();
        config.cButtons = button_list.len() as u32;
        config.pButtons = if button_list.is_empty() { std::ptr::null() } else { button_list.as_ptr() };
        config.cRadioButtons = radio_list.len() as u32;
        config.pRadioButtons = if radio_list.is_empty() { std::ptr::null() } else { radio_list.as_ptr() };
        config.nDefaultRadioButton = radios.first().map_or(0, |(id, _)| *id);
        let (mut button, mut radio, mut checked) = (0i32, 0i32, 0 as BOOL);
        let hr = TaskDialogIndirect(&config, &mut button, &mut radio, &mut checked);
        if !icon.is_null() {
            wm::DestroyIcon(icon);
        }
        (hr >= 0).then_some((button, radio, checked != 0))
    }
}

/// Asks how to install. None when the user cancelled.
pub fn ask() -> Option<Choice> {
    let (button, radio, autostart) = task_dialog(
        "BetterClipboard Setup",
        "Install BetterClipboard",
        &format!(
            "Version {}. Installs for your account only, no administrator rights needed. Nothing you copy leaves this PC.\n\nHow should your clipboard history open?",
            crate::install::VERSION
        ),
        &[(1, "Install")],
        &[
            (RADIO_WIN_V, "With Win+V (recommended)\nReplaces the Windows clipboard panel. Explorer restarts once."),
            (RADIO_KEEP, "Keep Win+V for Windows\nBetterClipboard opens with Win+Alt+V."),
        ],
        Some("Start BetterClipboard when I sign in"),
        TDCBF_CANCEL_BUTTON,
    )?;
    (button == 1).then_some(Choice { win_v: radio == RADIO_WIN_V, autostart })
}

/// Installs, applies the choice and starts the program. Returns a message for the user.
pub fn run(choice: &Choice) -> Result<String, String> {
    // Write the shortcut first, so the installed program starts with the right one.
    let path = crate::util::data_dir().join("settings.ini");
    let before = crate::system::windows_win_v_disabled();
    let mut settings = Settings::load(&path);
    settings.hotkey = if choice.win_v { Hotkey::WIN_V } else { Hotkey::DEFAULT };
    settings.save(&path).map_err(|e| format!("The settings could not be saved: {e}"))?;
    if choice.win_v {
        crate::system::set_windows_win_v_disabled(true)?;
    } else if crate::system::windows_win_v_disabled() && crate::util::data_dir_root().join("win-v-taken").exists() {
        // An earlier installation had taken Win+V; give it back.
        crate::system::set_windows_win_v_disabled(false)?;
    }
    let target = crate::install::install(choice.autostart)?;
    let _ = std::process::Command::new(&target).arg("--background").spawn();
    if crate::system::windows_win_v_disabled() != before {
        // Explorer reads DisabledHotkeys when it starts.
        crate::system::restart_explorer();
    }
    Ok(if choice.win_v {
        "Press Win+V to open your clipboard history. Right-click the tray icon for settings.".into()
    } else {
        "Press Win+Alt+V to open your clipboard history. Right-click the tray icon for settings.".into()
    })
}

pub fn finished(message: &str) {
    task_dialog("BetterClipboard Setup", "BetterClipboard is ready", message, &[], &[], None, TDCBF_OK_BUTTON);
}

pub fn failed(message: &str) {
    unsafe {
        wm::MessageBoxW(
            std::ptr::null_mut::<core::ffi::c_void>() as HWND,
            wide(message).as_ptr(),
            wide("BetterClipboard Setup").as_ptr(),
            wm::MB_ICONERROR,
        )
    };
}
