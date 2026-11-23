//! BetterClipboard entry point.
#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::process::ExitCode;

use bcb::app::{self, Remote};
use bcb::util::{self, wide};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

const HELP: &str = "BetterClipboard [option]

(no option)      Start, or open the history if it is already running
--background     Start without the welcome notification
--show           Open the history of the running instance
--settings       Open settings
--snippet        Create a pinned snippet
--quit           Quit the running instance
--install        Install for the current user (Start menu, sign-in start)
--uninstall      Remove the installation (--quiet: keep data, no questions)
--version        Show the version";

fn message(text: &str, flags: u32) -> i32 {
    unsafe { wm::MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("BetterClipboard").as_ptr(), flags) }
}

fn running_window() -> Option<windows_sys::Win32::Foundation::HWND> {
    let class = app::main_class();
    let window = unsafe { wm::FindWindowW(class.as_ptr(), std::ptr::null()) };
    (!window.is_null()).then_some(window)
}

fn forward(remote: Remote) -> bool {
    let Some(window) = running_window() else { return false };
    unsafe {
        let mut pid = 0u32;
        wm::GetWindowThreadProcessId(window, &mut pid);
        // Lets the running instance bring its window to the front.
        wm::AllowSetForegroundWindow(pid);
        wm::PostMessageW(window, app::WM_REMOTE, remote as usize, 0);
    }
    true
}

fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> ExitCode {
    // DLLs only ever come from System32.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--help") || has("-h") || has("/?") {
        message(HELP, wm::MB_ICONINFORMATION);
        return ExitCode::SUCCESS;
    }
    if has("--version") {
        message(&format!("BetterClipboard {}", bcb::install::VERSION), wm::MB_ICONINFORMATION);
        return ExitCode::SUCCESS;
    }
    if has("--install") {
        return match bcb::install::install() {
            Ok(target) => {
                let _ = std::process::Command::new(&target).spawn();
                if !has("--quiet") {
                    message(
                        "BetterClipboard is installed for your account and starts when you sign in.\n\nOpen your clipboard history with Win+Alt+V or the tray icon.",
                        wm::MB_ICONINFORMATION,
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                if !has("--quiet") {
                    message(&error, wm::MB_ICONERROR);
                }
                ExitCode::from(1)
            }
        };
    }
    if has("--uninstall") {
        let quiet = has("--quiet");
        if !quiet && message("Remove BetterClipboard from this PC?", wm::MB_YESNO | wm::MB_ICONQUESTION) != wm::IDYES {
            return ExitCode::from(1);
        }
        let remove_data = !quiet
            && message(
                "Also delete your saved clipboard entries and settings?\n\nChoose No to keep them for a later installation.",
                wm::MB_YESNO | wm::MB_ICONQUESTION,
            ) == wm::IDYES;
        bcb::install::uninstall(remove_data);
        if !quiet {
            message("BetterClipboard was removed.", wm::MB_ICONINFORMATION);
        }
        return ExitCode::SUCCESS;
    }

    let mutex_name = wide(&format!("Local\\BetterClipboard.Instance{}", util::profile()));
    let _mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    let remote = if has("--quit") {
        Some(Remote::Quit)
    } else if has("--settings") {
        Some(Remote::Settings)
    } else if has("--snippet") {
        Some(Remote::Snippet)
    } else if has("--show") || (args.is_empty() && already_running) {
        Some(Remote::Show)
    } else {
        None
    };
    if already_running {
        if let Some(remote) = remote {
            forward(remote);
        }
        return ExitCode::SUCCESS;
    }
    if remote == Some(Remote::Quit) {
        return ExitCode::SUCCESS;
    }

    let benchmark = has("--bench-popup") || has("--bench-search") || has("--startup-report");
    let options = app::Options { no_hotkey: has("--no-hotkey"), background: has("--background") || benchmark };
    // Relative paths handed to the shell must not resolve next to a downloaded file.
    if let Some(system) = std::env::var_os("SystemRoot") {
        let _ = std::env::set_current_dir(PathBuf::from(system).join("System32"));
    }
    if !app::start(&options) {
        message("BetterClipboard could not start.", wm::MB_ICONERROR);
        return ExitCode::from(1);
    }

    if let Some(path) = value_after(&args, "--startup-report") {
        bcb::bench::startup_report(&PathBuf::from(path));
        return ExitCode::SUCCESS;
    }
    if has("--bench-popup") {
        let runs = value_after(&args, "--bench-popup").and_then(|v| v.parse().ok()).unwrap_or(30);
        let items = value_after(&args, "--items").and_then(|v| v.parse().ok()).unwrap_or(25);
        let out = value_after(&args, "--report").unwrap_or_else(|| "bench-popup.txt".into());
        bcb::bench::popup(runs, items, &PathBuf::from(out));
        return ExitCode::SUCCESS;
    }
    if has("--bench-search") {
        let items = value_after(&args, "--items").and_then(|v| v.parse().ok()).unwrap_or(10_000);
        let out = value_after(&args, "--report").unwrap_or_else(|| "bench-search.txt".into());
        bcb::bench::search(items, &PathBuf::from(out));
        return ExitCode::SUCCESS;
    }
    let hotkey_log = value_after(&args, "--bench-hotkey-log");
    if hotkey_log.is_some() {
        bcb::bench::record_hotkeys();
    }
    match remote {
        Some(Remote::Settings) => app::perform(Some(app::Outside::Settings)),
        Some(Remote::Snippet) => app::perform(Some(app::Outside::Snippet)),
        Some(Remote::Show) => {
            app::with(|a| a.show_popup(app::Opened::Command));
        }
        _ => {}
    }
    app::run_message_loop();
    if let Some(path) = hotkey_log {
        bcb::bench::write_hotkey_log(&PathBuf::from(path));
    }
    ExitCode::SUCCESS
}
