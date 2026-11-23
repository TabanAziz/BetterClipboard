//! Per-user installation: no administrator rights, nothing outside the user's profile.
//!
//! `--install` copies the program to %LOCALAPPDATA%\Programs\BetterClipboard, adds a
//! Start menu shortcut, an "Installed apps" entry and the sign-in start. `--uninstall`
//! reverses every change, including the optional Win+V takeover.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::core::GUID;
use windows_sys::Win32::System::Com;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::system;
use crate::util::wide;

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\BetterClipboard";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("Programs").join("BetterClipboard")
}

fn start_menu_link() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join(r"Microsoft\Windows\Start Menu\Programs\BetterClipboard.lnk")
}

fn set_string(key: &str, name: &str, value: &str) -> Result<(), String> {
    let (k, n, v) = (wide(key), wide(name), wide(value));
    let status = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, v.as_ptr() as _, (v.len() * 2) as u32)
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("Could not write the registry value {name} (error {status})."))
    }
}

fn set_number(key: &str, name: &str, value: u32) -> Result<(), String> {
    let (k, n) = (wide(key), wide(name));
    let status =
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_DWORD, &value as *const u32 as _, 4) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("Could not write the registry value {name} (error {status})."))
    }
}

/// Creates a .lnk through the documented IShellLinkW / IPersistFile COM interfaces.
pub fn create_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    const SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
    const ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
    const IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
    const QUERY: usize = 0;
    const RELEASE: usize = 2;
    const SAVE: usize = 6;
    const SET_DESCRIPTION: usize = 7;
    const SET_DIRECTORY: usize = 9;
    const SET_PATH: usize = 20;
    type Raw = *mut c_void;
    type Query = unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> i32;
    type Release = unsafe extern "system" fn(Raw) -> u32;
    type SetText = unsafe extern "system" fn(Raw, *const u16) -> i32;
    type Save = unsafe extern "system" fn(Raw, *const u16, i32) -> i32;
    unsafe fn method<F: Copy>(object: Raw, slot: usize) -> F {
        std::mem::transmute_copy(&*(*(object as *const *const Raw)).add(slot))
    }
    let text = |p: &Path| wide(&p.to_string_lossy());
    let dir = target.parent().unwrap_or(target);
    let (file, path, folder, about) = (text(link), text(target), text(dir), wide("Clipboard history for Windows"));
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    unsafe {
        let initialised = Com::CoInitializeEx(std::ptr::null(), Com::COINIT_APARTMENTTHREADED as u32) >= 0;
        let mut object: Raw = std::ptr::null_mut();
        let mut hr = Com::CoCreateInstance(
            &SHELL_LINK,
            std::ptr::null_mut(),
            Com::CLSCTX_INPROC_SERVER,
            &ISHELL_LINK_W,
            &mut object,
        );
        if hr >= 0 {
            hr = method::<SetText>(object, SET_PATH)(object, path.as_ptr());
            method::<SetText>(object, SET_DIRECTORY)(object, folder.as_ptr());
            method::<SetText>(object, SET_DESCRIPTION)(object, about.as_ptr());
            let mut persist: Raw = std::ptr::null_mut();
            if hr >= 0 {
                hr = method::<Query>(object, QUERY)(object, &IPERSIST_FILE, &mut persist);
            }
            if hr >= 0 {
                hr = method::<Save>(persist, SAVE)(persist, file.as_ptr(), 1);
                method::<Release>(persist, RELEASE)(persist);
            }
            method::<Release>(object, RELEASE)(object);
        }
        if initialised {
            Com::CoUninitialize();
        }
        if hr >= 0 {
            Ok(())
        } else {
            Err(format!("The shortcut {} could not be created.", link.display()))
        }
    }
}

/// Asks a running instance to quit and waits for it (up to five seconds).
pub fn stop_running_instance() {
    let class = crate::app::main_class();
    unsafe {
        let window = wm::FindWindowW(class.as_ptr(), std::ptr::null());
        if window.is_null() {
            return;
        }
        wm::PostMessageW(window, crate::app::WM_REMOTE, crate::app::Remote::Quit as usize, 0);
        for _ in 0..100 {
            if wm::IsWindow(window) == 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
}

pub fn install() -> Result<PathBuf, String> {
    let source = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = install_dir();
    let target = dir.join("BetterClipboard.exe");
    stop_running_instance();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    if !same_file(&source, &target) {
        // Retry briefly: the old copy may still be closing.
        let mut last = None;
        for _ in 0..20 {
            match std::fs::copy(&source, &target) {
                Ok(_) => {
                    last = None;
                    break;
                }
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        if let Some(e) = last {
            return Err(format!("Could not copy the program to {}: {e}", target.display()));
        }
    }
    create_shortcut(&start_menu_link(), &target)?;
    let exe = target.display().to_string();
    let size_kb = std::fs::metadata(&target).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    set_string(UNINSTALL_KEY, "DisplayName", "BetterClipboard")?;
    set_string(UNINSTALL_KEY, "DisplayVersion", VERSION)?;
    set_string(UNINSTALL_KEY, "Publisher", "TabanAziz")?;
    set_string(UNINSTALL_KEY, "DisplayIcon", &format!("{exe},0"))?;
    set_string(UNINSTALL_KEY, "InstallLocation", &dir.display().to_string())?;
    set_string(UNINSTALL_KEY, "UninstallString", &format!("\"{exe}\" --uninstall"))?;
    set_string(UNINSTALL_KEY, "QuietUninstallString", &format!("\"{exe}\" --uninstall --quiet"))?;
    set_string(UNINSTALL_KEY, "URLInfoAbout", "https://github.com/TabanAziz/BetterClipboard")?;
    set_number(UNINSTALL_KEY, "NoModify", 1)?;
    set_number(UNINSTALL_KEY, "NoRepair", 1)?;
    set_number(UNINSTALL_KEY, "EstimatedSize", size_kb)?;
    let run = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("BetterClipboard");
    let value = wide(&format!("\"{exe}\" --background"));
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            run.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr() as _,
            (value.len() * 2) as u32,
        )
    };
    Ok(target)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

pub struct Uninstalled {
    pub removed_data: bool,
}

pub fn uninstall(remove_data: bool) -> Uninstalled {
    stop_running_instance();
    let _ = system::set_autostart(false);
    // Hand Win+V back only if BetterClipboard took it.
    if crate::util::data_dir_root().join("win-v-taken").exists() {
        let _ = system::set_windows_win_v_disabled(false);
    }
    let _ = std::fs::remove_file(start_menu_link());
    let key = wide(UNINSTALL_KEY);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) };
    if remove_data {
        let _ = std::fs::remove_dir_all(crate::util::data_dir_root());
    }
    // The running program cannot delete itself; a short-lived cmd removes it after exit.
    let dir = install_dir();
    if let Ok(exe) = std::env::current_exe() {
        if exe.parent().is_some_and(|p| same_file(p, &dir)) {
            let command =
                format!("/c ping 127.0.0.1 -n 3 > nul & del /f /q \"{}\" & rmdir \"{}\"", exe.display(), dir.display());
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd.exe").raw_arg(command).creation_flags(0x0800_0000).spawn();
        }
    }
    Uninstalled { removed_data: remove_data }
}
