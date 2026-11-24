//! Windows integration helpers: registry values, theme, shell links and process integrity.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY,
};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SW_SHOWNORMAL};

use crate::util::{from_wide, wide};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "BetterClipboard";
const EXPLORER_ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

fn read_dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
    let (key, name) = (wide(key), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as _,
            &mut size,
        )
    };
    (status == 0).then_some(value)
}

fn read_string(root: HKEY, key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as _,
            &mut size,
        )
    };
    (status == 0).then(|| from_wide(&buffer))
}

fn write_string(key: &str, name: &str, value: &str) -> Result<(), String> {
    let (key, name, data) = (wide(key), wide(name), wide(value));
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            data.as_ptr() as _,
            (data.len() * 2) as u32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "The registry value {name:?} could not be written (error {status}).",
            name = String::from_utf16_lossy(&name)
        ))
    }
}

fn delete_value(key: &str, name: &str) {
    let (key, name) = (wide(key), wide(name));
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
}

pub fn apps_use_light_theme() -> bool {
    read_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
        .is_none_or(|v| v != 0)
}

/// Windows accent colour as 0xRRGGBB.
pub fn accent_color() -> u32 {
    let mut color = 0u32;
    let mut opaque = 0i32;
    let ok = unsafe { windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor(&mut color, &mut opaque) } >= 0;
    if ok {
        color & 0xFF_FFFF
    } else {
        0x00_5F_B8
    }
}

/// State of the Windows clipboard history switch (Settings > System > Clipboard).
pub fn windows_history_enabled() -> Option<bool> {
    read_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Clipboard", "EnableClipboardHistory").map(|v| v != 0)
}

/// State of "Clipboard history across your devices" (Microsoft account sync).
pub fn windows_cloud_sync_enabled() -> Option<bool> {
    read_dword(HKEY_CURRENT_USER, r"Software\Microsoft\Clipboard", "EnableCloudClipboard").map(|v| v != 0)
}

/// True when a group policy turned clipboard history off for this PC or user.
pub fn history_blocked_by_policy() -> bool {
    let key = r"SOFTWARE\Policies\Microsoft\Windows\System";
    read_dword(HKEY_LOCAL_MACHINE, key, "AllowClipboardHistory") == Some(0)
        || read_dword(HKEY_CURRENT_USER, key, "AllowClipboardHistory") == Some(0)
}

fn quoted_exe() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\" --background")
}

pub fn autostart_enabled() -> bool {
    read_string(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).is_some()
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    if on {
        write_string(RUN_KEY, RUN_VALUE, &quoted_exe())
    } else {
        delete_value(RUN_KEY, RUN_VALUE);
        Ok(())
    }
}

/// Whether Explorer's own Win+V is switched off through the documented `DisabledHotkeys` value.
pub fn windows_win_v_disabled() -> bool {
    read_string(HKEY_CURRENT_USER, EXPLORER_ADVANCED, "DisabledHotkeys").is_some_and(|v| v.contains(['V', 'v']))
}

/// Adds or removes "V" in `DisabledHotkeys`, keeping any other letters the user set.
/// Explorer reads the value at sign-in, so the change applies after signing out.
pub fn set_windows_win_v_disabled(disabled: bool) -> Result<(), String> {
    let current = read_string(HKEY_CURRENT_USER, EXPLORER_ADVANCED, "DisabledHotkeys").unwrap_or_default();
    let others: String = current.chars().filter(|c| !c.eq_ignore_ascii_case(&'v')).collect();
    let next = if disabled { format!("{others}V") } else { others };
    let result = if next.is_empty() {
        delete_value(EXPLORER_ADVANCED, "DisabledHotkeys");
        Ok(())
    } else {
        write_string(EXPLORER_ADVANCED, "DisabledHotkeys", &next)
    };
    // Remember that BetterClipboard made the change, so uninstalling can undo it.
    let marker = crate::util::data_dir_root().join("win-v-taken");
    if result.is_ok() {
        if disabled {
            let _ = std::fs::create_dir_all(crate::util::data_dir_root());
            let _ = std::fs::write(&marker, b"1");
        } else {
            let _ = std::fs::remove_file(&marker);
        }
    }
    result
}

/// Ends and restarts Explorer for the current user, like Task Manager's "Restart" on
/// Windows Explorer. Explorer reads `DisabledHotkeys` when it starts, so this applies the
/// Win+V change without signing out. Open File Explorer windows close.
pub fn restart_explorer() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let session = format!("SESSION eq {}", current_session());
    let ended = std::process::Command::new(format!(r"{windows}\System32\taskkill.exe"))
        .args(["/f", "/im", "explorer.exe", "/fi", &session])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success());
    std::thread::sleep(std::time::Duration::from_millis(800));
    let started = std::process::Command::new(format!(r"{windows}\explorer.exe")).spawn().is_ok();
    ended && started
}

fn current_session() -> u32 {
    let mut session = 0u32;
    unsafe {
        windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            windows_sys::Win32::System::Threading::GetCurrentProcessId(),
            &mut session,
        )
    };
    session
}

/// Opens a URI or file with its default handler (ms-settings:, ms-phone:, https:, files).
pub fn open(target: &str) -> bool {
    let (verb, file) = (wide("open"), wide(target));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}

/// Opens the selected BetterExplorer installation, or Windows File Explorer.
pub fn reveal(path: &str) -> bool {
    let key = r"Software\Classes\Directory\shell";
    if std::path::Path::new(path).is_absolute()
        && read_string(HKEY_CURRENT_USER, key, "").as_deref() == Some("BetterExplorer.Open")
        && own_integrity().is_some_and(|level| level < 0x3000)
    {
        if let Some(exe) = read_string(
            HKEY_CURRENT_USER,
            r"Software\Classes\Directory\shell\BetterExplorer.Open",
            "BetterExplorerExecutable",
        ) {
            let exe = std::path::Path::new(&exe);
            if exe.is_absolute()
                && exe.file_name().is_some_and(|name| name.eq_ignore_ascii_case("BetterExplorer.exe"))
                && std::process::Command::new(exe).args(["--select", path]).spawn().is_ok()
            {
                return true;
            }
        }
    }
    let (verb, file, args) = (wide("open"), wide("explorer.exe"), wide(&format!("/select,\"{path}\"")));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}

fn token_integrity(token: HANDLE) -> Option<u32> {
    unsafe {
        let mut size = 0u32;
        GetTokenInformation(token, TokenIntegrityLevel, std::ptr::null_mut(), 0, &mut size);
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        if GetTokenInformation(token, TokenIntegrityLevel, buffer.as_mut_ptr() as _, size, &mut size) == 0 {
            return None;
        }
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(label.Label.Sid, count as u32 - 1))
    }
}

pub fn own_integrity() -> Option<u32> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let level = token_integrity(token);
        CloseHandle(token);
        level
    }
}

/// Integrity level of the process behind a window. None when it cannot be read, which
/// for a normal user usually means the process runs elevated.
pub fn window_integrity(window: HWND) -> Option<u32> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut token: HANDLE = std::ptr::null_mut();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token) != 0;
        CloseHandle(process);
        if !opened {
            return None;
        }
        let level = token_integrity(token);
        CloseHandle(token);
        level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_integrity_is_readable() {
        // Medium (0x2000) for a normal user, High (0x3000) when tests run elevated.
        assert!(own_integrity().unwrap() >= 0x1000);
    }
}
