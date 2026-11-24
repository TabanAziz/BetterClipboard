//! The running app: a hidden main window that owns the hotkey, the tray icon and the
//! history, plus the panel window. All state lives on the UI thread.
//!
//! Window procedures borrow the state only briefly. Anything that runs its own message
//! loop (menus, file dialogs) is executed after the borrow ends, see [`perform`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, TrackMouseEvent, UnregisterHotKey, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::clipboard::{self, CaptureConfig, Command, Event, Service, Skip};
use crate::model::{Added, Content, History, Item, Limits, Origin};
use crate::paste;
use crate::settings::{Hotkey, Settings, Theme};
use crate::store;
use crate::system;
use crate::tray::{self, Tray};
use crate::ui::popup::{self, Action, Context, PasteMode, Popup};
use crate::ui::settings_window;
use crate::util::{self, wide};

pub const WM_JOB: u32 = wm::WM_APP + 20;
const NIN_KEYSELECT: u32 = windows_sys::Win32::UI::Shell::NIN_SELECT | windows_sys::Win32::UI::Shell::NINF_KEY;
/// Sent by a second instance: wParam is a [`Remote`] code.
pub const WM_REMOTE: u32 = wm::WM_APP + 30;

const HOTKEY_ID: i32 = 1;
const SAVE_TIMER: usize = 1;
const PRUNE_TIMER: usize = 2;
const PASTE_TIMER: usize = 3;
const TOAST_TIMER: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Remote {
    Show = 1,
    Settings = 2,
    Quit = 3,
    Snippet = 4,
    /// Re-read settings.ini (written by `--use-win-v`).
    Reload = 5,
}

impl Remote {
    fn from(code: usize) -> Option<Remote> {
        Some(match code {
            1 => Remote::Show,
            2 => Remote::Settings,
            3 => Remote::Quit,
            4 => Remote::Snippet,
            5 => Remote::Reload,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opened {
    Hotkey,
    Tray,
    Command,
}

pub enum Job {
    Loaded(store::Loaded),
    Saved(Result<(), String>),
    FileWritten { path: PathBuf, open: bool, result: Result<(), String> },
}

enum SaverCommand {
    Load(PathBuf),
    Save(PathBuf, Vec<Item>),
    Quit,
}

struct Saver {
    commands: Sender<SaverCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Saver {
    fn start(main: HWND, jobs: Sender<Job>) -> Saver {
        let (tx, rx) = channel::<SaverCommand>();
        let main = main as isize;
        let thread = std::thread::Builder::new()
            .name("storage".into())
            .spawn(move || {
                let post = |job: Job| {
                    if jobs.send(job).is_ok() {
                        unsafe { wm::PostMessageW(main as HWND, WM_JOB, 0, 0) };
                    }
                };
                while let Ok(command) = rx.recv() {
                    match command {
                        SaverCommand::Load(path) => post(Job::Loaded(store::load(&path))),
                        SaverCommand::Save(path, mut items) => {
                            // Only the newest snapshot matters.
                            let mut quit = false;
                            while let Ok(next) = rx.try_recv() {
                                match next {
                                    SaverCommand::Save(_, newer) => items = newer,
                                    SaverCommand::Quit => quit = true,
                                    SaverCommand::Load(_) => {}
                                }
                            }
                            post(Job::Saved(store::save(&path, &items)));
                            if quit {
                                break;
                            }
                        }
                        SaverCommand::Quit => break,
                    }
                }
            })
            .expect("storage thread");
        Saver { commands: tx, thread: Some(thread) }
    }

    fn send(&self, command: SaverCommand) {
        let _ = self.commands.send(command);
    }

    /// Waits until everything queued is written.
    fn finish(&mut self) {
        let _ = self.commands.send(SaverCommand::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub captured: u64,
    pub skipped_private: u64,
    pub skipped_excluded: u64,
    pub skipped_large: u64,
    pub skipped_locked: u64,
}

/// A paste waiting for the target window to take the keyboard focus back.
#[derive(Clone)]
struct PendingPaste {
    target: HWND,
    attempts: u32,
    /// When the target became the foreground window again (QPC seconds).
    focused_at: Option<f64>,
    /// Type this text instead of pressing Ctrl+V (phone items, see `choose`).
    text: Option<Arc<str>>,
}

pub struct App {
    pub main: HWND,
    pub settings: Settings,
    settings_path: PathBuf,
    history_path: PathBuf,
    pub history: History,
    clip: Option<Service>,
    clip_events: Receiver<Event>,
    pub popup: Popup,
    tray: Tray,
    target: HWND,
    pending_paste: Option<PendingPaste>,
    hotkey_registered: bool,
    pub notice: Option<String>,
    pub policy_blocked: bool,
    saver: Saver,
    jobs: Receiver<Job>,
    jobs_tx: Sender<Job>,
    pub loaded: bool,
    pub stats: Stats,
    taskbar_created: u32,
    pub settings_window: HWND,
    pub no_hotkey: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static LAST_FOREGROUND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// Runs `f` with the app state, unless it is already borrowed further up the stack
/// (a nested window message); then the message falls back to default handling.
pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

pub fn main_class() -> Vec<u16> {
    wide(&format!("BetterClipboard.Main{}", util::profile()))
}

fn capture_config(settings: &Settings, policy_blocked: bool) -> CaptureConfig {
    CaptureConfig {
        enabled: settings.history_enabled && !settings.paused && !policy_blocked,
        max_bytes: settings.max_item_bytes(),
        excluded: settings.excluded_apps.clone(),
    }
}

pub struct Options {
    pub no_hotkey: bool,
    pub background: bool,
}

/// Creates windows, threads and the tray icon. Returns false if start-up failed.
pub fn start(options: &Options) -> bool {
    let data = util::data_dir();
    let _ = std::fs::create_dir_all(&data);
    let settings_path = data.join("settings.ini");
    let history_path = data.join("history.bin");
    let settings = Settings::load(&settings_path);
    let policy_blocked = system::history_blocked_by_policy();

    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = main_class();
    let main = unsafe {
        let mut definition: wm::WNDCLASSEXW = std::mem::zeroed();
        definition.cbSize = std::mem::size_of_val(&definition) as u32;
        definition.hInstance = module;
        definition.lpszClassName = class.as_ptr();
        definition.lpfnWndProc = Some(main_proc);
        wm::RegisterClassExW(&definition);
        wm::CreateWindowExW(
            wm::WS_EX_TOOLWINDOW,
            class.as_ptr(),
            wide("BetterClipboard").as_ptr(),
            wm::WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    };
    if main.is_null() {
        return false;
    }
    let panel_class = popup::register_class(Some(popup_proc));
    let popup = Popup::create(&panel_class, settings.theme);
    let (clip, clip_events) = Service::start(main, capture_config(&settings, policy_blocked));
    let (jobs_tx, jobs) = channel();
    let saver = Saver::start(main, jobs_tx.clone());
    saver.send(SaverCommand::Load(history_path.clone()));

    let mut app = App {
        main,
        history: History::new(Limits {
            max_items: settings.max_items as usize,
            group_duplicates: settings.group_duplicates,
        }),
        settings,
        settings_path,
        history_path,
        clip: Some(clip),
        clip_events,
        popup,
        tray: Tray::new(main),
        target: std::ptr::null_mut(),
        pending_paste: None,
        hotkey_registered: false,
        notice: None,
        policy_blocked,
        saver,
        jobs,
        jobs_tx,
        loaded: false,
        stats: Stats::default(),
        taskbar_created: unsafe { wm::RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        settings_window: std::ptr::null_mut(),
        no_hotkey: options.no_hotkey,
    };
    if !app.clip.as_ref().is_some_and(Service::is_running) {
        app.notice = Some("Windows did not allow listening to the clipboard.".into());
    }
    app.register_hotkey();
    let tip = app.tray_tip();
    app.tray.add(app.settings.paused, &tip);
    app.popup.set_theme(app.settings.theme);
    app.popup.prewarm_emoji();
    unsafe {
        wm::SetTimer(main, PRUNE_TIMER, 60 * 60 * 1000, None);
        // Foreground changes tell us where to paste when the panel opens from the tray.
        SetWinEventHook(
            wm::EVENT_SYSTEM_FOREGROUND,
            wm::EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(foreground_changed),
            0,
            0,
            wm::WINEVENT_OUTOFCONTEXT | wm::WINEVENT_SKIPOWNPROCESS,
        );
    }
    let first_run = !app.settings_path.exists();
    if first_run {
        let _ = app.settings.save(&app.settings_path);
    }
    if first_run || !options.background {
        let message = match (&app.notice, app.hotkey_registered) {
            (Some(notice), _) => notice.clone(),
            (None, true) => format!("Press {} to open your clipboard history.", app.settings.hotkey.label()),
            (None, false) => "Click the tray icon to open your clipboard history.".into(),
        };
        app.tray.notify("BetterClipboard is running", &message);
    }
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    true
}

pub fn run_message_loop() {
    unsafe {
        let mut message: wm::MSG = std::mem::zeroed();
        while wm::GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            // Tab and Enter navigation in the settings and snippet windows.
            if settings_window::pretranslate(&message) {
                continue;
            }
            let snippet = crate::ui::dialogs::SNIPPET_WINDOW.with(|d| d.get());
            if snippet != 0
                && (message.hwnd as isize == snippet || wm::IsChild(snippet as HWND, message.hwnd) != 0)
                && wm::IsDialogMessageW(snippet as HWND, &message) != 0
            {
                continue;
            }
            wm::TranslateMessage(&message);
            wm::DispatchMessageW(&message);
        }
    }
    // Final save and thread shutdown.
    APP.with(|cell| {
        if let Some(mut app) = cell.borrow_mut().take() {
            app.shutdown();
        }
    });
}

/// Saves and stops the worker threads without a message loop (benchmark runs).
pub fn finish() {
    APP.with(|cell| {
        if let Some(mut app) = cell.borrow_mut().take() {
            app.shutdown();
        }
    });
}

pub fn pump_pending() {
    unsafe {
        let mut message: wm::MSG = std::mem::zeroed();
        while wm::PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, wm::PM_REMOVE) != 0 {
            wm::TranslateMessage(&message);
            wm::DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn foreground_changed(_: HWINEVENTHOOK, _: u32, window: HWND, _: i32, _: i32, _: u32, _: u32) {
    if window.is_null() {
        return;
    }
    let mut class = [0u16; 64];
    let n = wm::GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
    // The taskbar and its flyouts are never paste targets.
    const SHELL: [&str; 5] = [
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "NotifyIconOverflowWindow",
        "TopLevelWindowForOverflowXamlIsland",
        "XamlExplorerHostIslandWindow",
    ];
    if !SHELL.contains(&class.as_str()) {
        LAST_FOREGROUND.with(|c| c.set(window as isize));
    }
}

impl App {
    fn tray_tip(&self) -> String {
        let state = if self.policy_blocked {
            "turned off by your organization"
        } else if !self.settings.history_enabled {
            "history is off"
        } else if self.settings.paused {
            "recording paused"
        } else {
            "recording"
        };
        format!("BetterClipboard: {state}")
    }

    fn refresh_tray(&self) {
        self.tray.update(self.settings.paused || !self.settings.history_enabled, &self.tray_tip());
    }

    pub fn register_hotkey(&mut self) {
        unsafe {
            if self.hotkey_registered {
                UnregisterHotKey(self.main, HOTKEY_ID);
                self.hotkey_registered = false;
            }
        }
        if self.no_hotkey {
            return;
        }
        let hotkey = self.settings.hotkey;
        self.hotkey_registered =
            unsafe { RegisterHotKey(self.main, HOTKEY_ID, hotkey.register_flags(), hotkey.vk) } != 0;
        self.notice = if self.hotkey_registered {
            None
        } else if hotkey == Hotkey::WIN_V && !system::windows_win_v_disabled() {
            Some("Win+V still belongs to Windows. See Settings.".into())
        } else {
            Some(format!("{} is taken by another app. Pick another shortcut in Settings.", hotkey.label()))
        };
    }

    fn redraw_popup(&mut self) {
        if self.popup.visible {
            let ctx = Context {
                history: &self.history,
                recent: &self.settings.recent,
                history_enabled: self.settings.history_enabled && !self.policy_blocked,
                paused: self.settings.paused,
                policy_blocked: self.policy_blocked,
                notice: self.notice.as_deref(),
                now: util::now_ms(),
            };
            self.popup.rebuild(&ctx);
            self.popup.invalidate();
        }
    }

    pub fn show_popup(&mut self, opened: Opened) {
        if self.popup.visible {
            self.popup.hide();
            return;
        }
        let foreground = unsafe { wm::GetForegroundWindow() };
        let own = |w: HWND| w == self.main || w == self.popup.hwnd || w == self.settings_window;
        self.target = match opened {
            Opened::Hotkey if !foreground.is_null() && !own(foreground) => foreground,
            _ => LAST_FOREGROUND.with(|c| c.get()) as HWND,
        };
        let (anchor, below) = if opened == Opened::Hotkey { caret_anchor(self.target) } else { (None, 0) };
        let anchor = anchor.unwrap_or_else(|| {
            let mut p = POINT { x: 0, y: 0 };
            unsafe { wm::GetCursorPos(&mut p) };
            p
        });
        let ctx = Context {
            history: &self.history,
            recent: &self.settings.recent,
            history_enabled: self.settings.history_enabled && !self.policy_blocked,
            paused: self.settings.paused,
            policy_blocked: self.policy_blocked,
            notice: self.notice.as_deref(),
            now: util::now_ms(),
        };
        self.popup.show(anchor, below, opened == Opened::Tray, &ctx);
    }

    fn schedule_save(&self) {
        unsafe { wm::SetTimer(self.main, SAVE_TIMER, 400, None) };
    }

    fn persisted_items(&self) -> Vec<Item> {
        if !self.settings.save_pinned {
            return Vec::new();
        }
        self.history.items().iter().filter(|i| i.pinned || self.settings.keep_history).cloned().collect()
    }

    fn save_now(&self) {
        // Nothing is written before the saved file was read, so a slow load never loses it.
        if self.loaded {
            self.saver.send(SaverCommand::Save(self.history_path.clone(), self.persisted_items()));
        }
    }

    fn configure_capture(&self) {
        if let Some(clip) = &self.clip {
            clip.send(Command::Configure(capture_config(&self.settings, self.policy_blocked)));
        }
    }

    fn prune(&mut self) {
        if self.settings.retention_days > 0 {
            let cutoff = util::now_ms() - self.settings.retention_days as i64 * 86_400_000;
            if !self.history.prune_older_than(cutoff).is_empty() {
                self.schedule_save();
                self.redraw_popup();
            }
        }
    }

    fn on_clip_events(&mut self) {
        while let Ok(event) = self.clip_events.try_recv() {
            match event {
                Event::Captured(captured) => {
                    if !self.settings.history_enabled || self.settings.paused || self.policy_blocked {
                        continue;
                    }
                    let id = self.history.allocate_id();
                    let item = Item::new(
                        id,
                        captured.content,
                        captured.image_print,
                        captured.app,
                        captured.origin,
                        util::now_ms(),
                    );
                    let mut dropped = Vec::new();
                    let added = self.history.add(item, &mut dropped);
                    self.stats.captured += 1;
                    let touches_saved = match added {
                        Added::Merged(id) => self.history.get(id).is_some_and(|i| i.pinned),
                        Added::New(_) => false,
                    };
                    if self.settings.keep_history || touches_saved {
                        self.schedule_save();
                    }
                    self.redraw_popup();
                }
                Event::Skipped(skip) => match skip {
                    Skip::AppAskedNotToRecord => self.stats.skipped_private += 1,
                    Skip::Excluded(_) => self.stats.skipped_excluded += 1,
                    Skip::TooLarge(_) => self.stats.skipped_large += 1,
                    Skip::Locked => self.stats.skipped_locked += 1,
                    Skip::Unsupported => {}
                },
                Event::Written { id: _, paste, result } => match result {
                    Err(error) => self.tray.notify("Could not copy", &error),
                    Ok(()) if paste => self.begin_paste(),
                    Ok(()) => {}
                },
            }
        }
    }

    fn begin_paste(&mut self) {
        self.begin_paste_with(None);
    }

    /// Pastes with Ctrl+V, or types `text` when given.
    fn begin_paste_with(&mut self, text: Option<Arc<str>>) {
        match paste::check_target(self.target) {
            Ok(()) => {
                self.pending_paste = Some(PendingPaste { target: self.target, attempts: 0, focused_at: None, text });
                self.try_paste();
            }
            Err(paste::Blocked::Elevated) => {
                let app = clipboard::window_process_name(self.target);
                let name = crate::model::app_label(&app);
                self.tray.notify(
                    "Copied to the clipboard",
                    &format!("{} runs as administrator, so Windows does not let other apps paste into it. Press Ctrl+V there.", if name.is_empty() { "The app".to_string() } else { name }),
                );
            }
            Err(paste::Blocked::TargetClosed) => {
                self.tray.notify("Copied to the clipboard", "Press Ctrl+V where you want to paste it.");
            }
        }
    }

    fn try_paste(&mut self) {
        let Some(mut pending) = self.pending_paste.clone() else { return };
        let target = pending.target;
        if paste::focus(target) {
            // Browsers and Electron apps restore the focus to their text field a little
            // after the window is active; a Ctrl+V sent before that is lost.
            let now = util::qpc_seconds();
            let since = *pending.focused_at.get_or_insert(now);
            let ready = paste::has_keyboard_focus(target) && now - since >= 0.06;
            if ready || now - since >= 0.3 {
                self.pending_paste = None;
                unsafe { wm::KillTimer(self.main, PASTE_TIMER) };
                let sent = match &pending.text {
                    Some(text) => paste::type_text(text),
                    None => paste::send_ctrl_v(),
                };
                if !sent {
                    self.tray.notify("Copied to the clipboard", "Windows blocked the paste. Press Ctrl+V yourself.");
                }
            } else {
                self.pending_paste = Some(pending);
                unsafe { wm::SetTimer(self.main, PASTE_TIMER, 15, None) };
            }
        } else if pending.attempts >= 12 {
            self.pending_paste = None;
            unsafe { wm::KillTimer(self.main, PASTE_TIMER) };
            self.tray.notify(
                "Copied to the clipboard",
                "The previous window could not be activated. Press Ctrl+V to paste.",
            );
        } else {
            pending.attempts += 1;
            pending.focused_at = None;
            self.pending_paste = Some(pending);
            unsafe { wm::SetTimer(self.main, PASTE_TIMER, 15, None) };
        }
    }

    fn on_jobs(&mut self) {
        while let Ok(job) = self.jobs.try_recv() {
            match job {
                Job::Loaded(loaded) => {
                    match loaded {
                        store::Loaded::Items(items) => {
                            for mut item in items {
                                // Windows starts with an empty history; only pinned entries survive
                                // unless the user asked to keep everything.
                                if !item.pinned && !self.settings.keep_history {
                                    continue;
                                }
                                if self.history.get(item.id).is_some() {
                                    item.id = self.history.allocate_id();
                                }
                                self.history.push_loaded(item);
                            }
                            let mut dropped = Vec::new();
                            self.history.enforce_limit(&mut dropped);
                        }
                        store::Loaded::Missing => {}
                        store::Loaded::Damaged(reason) => self.tray.notify(
                            "Saved clipboard entries could not be read",
                            &format!("The file was kept next to the new one. ({reason})"),
                        ),
                    }
                    self.loaded = true;
                    self.prune();
                    // Writes the cleaned list (drops unpinned entries if keeping history was turned off).
                    self.save_now();
                    self.redraw_popup();
                }
                Job::Saved(Err(error)) => self.tray.notify("Clipboard entries were not saved", &error),
                Job::Saved(Ok(())) => {}
                Job::FileWritten { path, open, result } => match result {
                    Ok(()) if open => {
                        system::open(&path.display().to_string());
                    }
                    Ok(()) => {
                        let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if self.popup.visible {
                            self.popup.show_toast(&format!("Saved as {shown}"), util::now_ms());
                            unsafe { wm::SetTimer(self.popup.hwnd, TOAST_TIMER, 2600, None) };
                        } else {
                            self.tray.notify("Image saved", &path.display().to_string());
                        }
                    }
                    Err(error) => self.tray.notify("The image was not saved", &error),
                },
            }
        }
    }

    /// Handles a panel action. Returns work that must run without the state borrowed.
    pub fn apply(&mut self, action: Action) -> Option<Outside> {
        match action {
            Action::Hide => {
                let had_focus = unsafe { wm::GetForegroundWindow() } == self.popup.hwnd;
                self.popup.hide();
                // Like Win+V: Escape returns to the window the panel was opened from.
                if had_focus && !self.target.is_null() && unsafe { wm::IsWindow(self.target) } != 0 {
                    unsafe { wm::SetForegroundWindow(self.target) };
                }
            }
            Action::Choose { id, mode } => self.choose(id, mode),
            Action::Menu { id, x, y } => return Some(Outside::ItemMenu { id, x, y }),
            Action::SaveImage(id) => return Some(Outside::SaveImage(id)),
            Action::Delete(id) => {
                if self.history.remove(id) {
                    remove_view_file(id);
                    self.schedule_save();
                    self.redraw_popup();
                }
            }
            Action::Pin(id, pinned) => {
                if self.history.set_pinned(id, pinned) {
                    self.schedule_save();
                    self.redraw_popup();
                }
            }
            Action::ClearAll => self.clear_all(false),
            Action::OpenSettings => {
                self.popup.hide();
                return Some(Outside::Settings);
            }
            Action::Insert(text) => {
                self.popup.hide();
                // Typed as Unicode input, so the clipboard and its history stay untouched.
                if paste::focus(self.target) && !paste::type_text(&text) {
                    self.tray
                        .notify("Could not insert", "Windows blocked the input. The app may run as administrator.");
                }
                self.settings.recent.retain(|r| *r != text);
                self.settings.recent.insert(0, text);
                self.settings.recent.truncate(crate::settings::RECENT_LIMIT);
                let _ = self.settings.save(&self.settings_path);
            }
            Action::Primary => {
                let mut next = self.settings.clone();
                if !next.history_enabled {
                    next.history_enabled = true;
                    next.paused = false;
                } else {
                    next.paused = false;
                }
                self.apply_settings(next);
            }
        }
        None
    }

    fn choose(&mut self, id: u64, mode: PasteMode) {
        let Some(item) = self.history.get(id) else { return };
        let content = Arc::clone(&item.content);
        let keep_local = item.origin == Origin::Phone;
        let fingerprint = item.fingerprint;
        let paste = mode != PasteMode::CopyOnly && self.settings.paste_on_select;
        // Phone Link sends every change of the PC clipboard to the phone. Text that came
        // from the phone is therefore typed in directly, without touching the clipboard.
        let phone_text = (keep_local && paste && content.picture.is_none() && content.files.is_empty())
            .then(|| content.preview_text())
            .filter(|t| !t.is_empty() && t.chars().count() <= 20_000);
        self.popup.hide();
        self.history.promote(id, util::now_ms());
        if self.settings.keep_history || self.history.get(id).is_some_and(|i| i.pinned) {
            self.schedule_save();
        }
        if let Some(text) = phone_text {
            self.begin_paste_with(Some(Arc::from(text.as_str())));
            return;
        }
        if let Some(clip) = &self.clip {
            clip.send(Command::Write {
                id,
                content,
                plain_text: mode == PasteMode::PlainText,
                paste,
                keep_local,
                fingerprint,
            });
        }
    }

    pub fn clear_all(&mut self, also_clipboard: bool) {
        for id in self.history.clear_unpinned() {
            remove_view_file(id);
        }
        if also_clipboard {
            if let Some(clip) = &self.clip {
                clip.send(Command::Clear);
            }
        }
        self.schedule_save();
        self.redraw_popup();
    }

    pub fn apply_settings(&mut self, next: Settings) {
        let previous = std::mem::replace(&mut self.settings, next);
        self.history.limits =
            Limits { max_items: self.settings.max_items as usize, group_duplicates: self.settings.group_duplicates };
        let mut dropped = Vec::new();
        self.history.enforce_limit(&mut dropped);
        if previous.history_enabled && !self.settings.history_enabled {
            // Like Windows: turning history off clears it, pinned entries stay.
            self.history.clear_unpinned();
        }
        self.configure_capture();
        if previous.hotkey != self.settings.hotkey || self.notice.is_some() {
            self.register_hotkey();
        }
        if previous.theme != self.settings.theme {
            self.refresh_theme();
        }
        self.save_settings();
        self.prune();
        self.schedule_save();
        self.refresh_tray();
        self.redraw_popup();
    }

    pub fn set_theme(&mut self, theme: Theme) {
        if self.settings.theme != theme {
            self.settings.theme = theme;
            self.refresh_theme();
            self.save_settings();
        }
    }

    fn refresh_theme(&mut self) {
        self.popup.set_theme(self.settings.theme);
        crate::ui::dialogs::refresh_snippet_theme(self.settings.theme);
    }

    fn save_settings(&self) {
        if let Err(error) = self.settings.save(&self.settings_path) {
            self.tray.notify("Settings were not saved", &error.to_string());
        }
    }

    pub fn add_snippet(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let id = self.history.allocate_id();
        let content = Content { text: Some(text), ..Default::default() };
        let item = Item::new(id, content, None, String::new(), Origin::Snippet, util::now_ms());
        let mut dropped = Vec::new();
        let added = self.history.add(item, &mut dropped);
        if let Added::Merged(existing) = added {
            self.history.set_pinned(existing, true);
        }
        self.schedule_save();
        self.redraw_popup();
    }

    fn on_tray_command(&mut self, command: usize) -> Option<Outside> {
        match command {
            tray::menu::OPEN => self.show_popup(Opened::Tray),
            tray::menu::PAUSE => {
                let mut next = self.settings.clone();
                next.paused = !next.paused;
                self.apply_settings(next);
            }
            tray::menu::HISTORY => {
                let mut next = self.settings.clone();
                next.history_enabled = !next.history_enabled;
                self.apply_settings(next);
            }
            tray::menu::CLEAR => self.clear_all(false),
            tray::menu::SNIPPET => return Some(Outside::Snippet),
            tray::menu::SETTINGS => return Some(Outside::Settings),
            tray::menu::PHONE_LINK => {
                if !system::open("ms-phone:") {
                    system::open("ms-settings:mobile-devices");
                }
            }
            tray::menu::WINDOWS_SETTINGS => {
                system::open("ms-settings:clipboard");
            }
            tray::menu::EXIT => unsafe {
                wm::DestroyWindow(self.main);
            },
            _ => {}
        }
        None
    }

    fn shutdown(&mut self) {
        unsafe { UnregisterHotKey(self.main, HOTKEY_ID) };
        self.tray.remove();
        self.save_now();
        self.saver.finish();
        self.clip.take();
        let _ = std::fs::remove_dir_all(view_dir());
    }

    pub fn jobs_sender(&self) -> Sender<Job> {
        self.jobs_tx.clone()
    }
}

/// Work that runs its own message loop and therefore must not hold the state borrow.
pub enum Outside {
    ItemMenu { id: u64, x: i32, y: i32 },
    SaveImage(u64),
    TrayMenu { x: i32, y: i32 },
    Settings,
    Snippet,
}

pub fn perform(outside: Option<Outside>) {
    let Some(outside) = outside else { return };
    match outside {
        Outside::ItemMenu { id, x, y } => item_menu(id, x, y),
        Outside::SaveImage(id) => save_image(id),
        Outside::TrayMenu { x, y } => {
            let Some((main, hotkey, paused, enabled)) = with(|app| {
                (
                    app.main,
                    app.settings.hotkey.label(),
                    app.settings.paused,
                    app.settings.history_enabled && !app.policy_blocked,
                )
            }) else {
                return;
            };
            let state = tray::MenuState { hotkey: &hotkey, paused, history_enabled: enabled };
            let command = tray::show_menu(main, x, y, &state);
            let next = with(|app| app.on_tray_command(command)).flatten();
            perform(next);
        }
        Outside::Settings => settings_window::open_settings(),
        Outside::Snippet => crate::ui::dialogs::open_snippet(),
    }
}

fn view_dir() -> PathBuf {
    util::data_dir().join("view")
}

fn remove_view_file(id: u64) {
    let _ = std::fs::remove_file(view_dir().join(format!("{id}.png")));
}

mod item_menu_ids {
    pub const PASTE: usize = 1;
    pub const PLAIN: usize = 2;
    pub const COPY: usize = 3;
    pub const PIN: usize = 4;
    pub const DELETE: usize = 5;
    pub const SAVE: usize = 6;
    pub const OPEN: usize = 7;
    pub const REVEAL: usize = 8;
    pub const CLEAR: usize = 9;
}

fn item_menu(id: u64, x: i32, y: i32) {
    use item_menu_ids::*;
    let Some((popup, pinned, has_image, first_file, has_text)) = with(|app| {
        let item = app.history.get(id)?;
        app.popup.modal = true;
        Some((
            app.popup.hwnd,
            item.pinned,
            item.content.picture.is_some(),
            item.content.files.first().cloned(),
            !item.content.preview_text().trim().is_empty(),
        ))
    })
    .flatten() else {
        return;
    };
    let chosen = unsafe {
        let menu = wm::CreatePopupMenu();
        let add = |id: usize, text: &str, enabled: bool| {
            let flags = if enabled { wm::MF_STRING } else { wm::MF_STRING | wm::MF_GRAYED };
            wm::AppendMenuW(menu, flags, id, wide(text).as_ptr());
        };
        add(PASTE, "Paste\tEnter", true);
        add(PLAIN, "Paste as plain text\tShift+Enter", has_text);
        add(COPY, "Copy only\tCtrl+Enter", true);
        wm::AppendMenuW(menu, wm::MF_SEPARATOR, 0, std::ptr::null());
        add(PIN, if pinned { "Unpin\tCtrl+P" } else { "Pin\tCtrl+P" }, true);
        add(DELETE, "Delete\tDel", true);
        if has_image {
            wm::AppendMenuW(menu, wm::MF_SEPARATOR, 0, std::ptr::null());
            add(SAVE, "Save image as...\tCtrl+S", true);
            add(OPEN, "Open image", true);
        }
        if first_file.is_some() {
            add(REVEAL, "Show in File Explorer", true);
        }
        wm::AppendMenuW(menu, wm::MF_SEPARATOR, 0, std::ptr::null());
        add(CLEAR, "Clear all (keeps pinned)", true);
        let chosen = wm::TrackPopupMenuEx(menu, wm::TPM_RETURNCMD | wm::TPM_RIGHTBUTTON, x, y, popup, std::ptr::null());
        wm::DestroyMenu(menu);
        chosen as usize
    };
    let outside = with(|app| {
        app.popup.modal = false;
        let action = match chosen {
            PASTE => Some(Action::Choose { id, mode: PasteMode::Default }),
            PLAIN => Some(Action::Choose { id, mode: PasteMode::PlainText }),
            COPY => Some(Action::Choose { id, mode: PasteMode::CopyOnly }),
            PIN => Some(Action::Pin(id, !pinned)),
            DELETE => Some(Action::Delete(id)),
            SAVE => Some(Action::SaveImage(id)),
            CLEAR => Some(Action::ClearAll),
            OPEN => {
                open_image(app, id);
                None
            }
            REVEAL => {
                if let Some(file) = &first_file {
                    app.popup.hide();
                    system::reveal(file);
                }
                None
            }
            _ => None,
        };
        // A menu closed by clicking outside the panel leaves it without focus.
        if app.popup.visible && unsafe { wm::GetForegroundWindow() } != app.popup.hwnd && chosen == 0 {
            app.popup.hide();
        }
        action.and_then(|a| app.apply(a))
    })
    .flatten();
    perform(outside);
}

fn open_image(app: &mut App, id: u64) {
    let Some(png) = app.history.get(id).and_then(|i| i.content.picture.as_ref()).map(|p| Arc::clone(&p.png)) else {
        return;
    };
    let path = view_dir().join(format!("{id}.png"));
    let jobs = app.jobs_sender();
    let main = app.main as isize;
    app.popup.hide();
    std::thread::spawn(move || {
        let result = std::fs::create_dir_all(view_dir())
            .and_then(|_| std::fs::write(&path, png.as_slice()))
            .map_err(|e| e.to_string());
        if jobs.send(Job::FileWritten { path, open: true, result }).is_ok() {
            unsafe { wm::PostMessageW(main as HWND, WM_JOB, 0, 0) };
        }
    });
}

fn save_image(id: u64) {
    let Some((popup, png, created)) = with(|app| {
        let item = app.history.get(id)?;
        let png = Arc::clone(&item.content.picture.as_ref()?.png);
        app.popup.modal = true;
        Some((app.popup.hwnd, png, item.created_ms))
    })
    .flatten() else {
        return;
    };
    let suggested = format!("Clipboard {}", crate::ui::dialogs::local_timestamp(created));
    let chosen = crate::ui::dialogs::save_file_dialog(popup, &suggested);
    let jobs_main = with(|app| {
        app.popup.modal = false;
        if app.popup.visible {
            unsafe { wm::SetForegroundWindow(app.popup.hwnd) };
        }
        (app.jobs_sender(), app.main as isize)
    });
    let (Some(path), Some((jobs, main))) = (chosen, jobs_main) else { return };
    std::thread::spawn(move || {
        let is_bmp = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bmp"));
        let result = if is_bmp {
            crate::image::Bgra::from_png(&png)
                .map(|b| b.to_bmp_file())
                .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()))
        } else {
            std::fs::write(&path, png.as_slice()).map_err(|e| e.to_string())
        };
        if jobs.send(Job::FileWritten { path, open: false, result }).is_ok() {
            unsafe { wm::PostMessageW(main as HWND, WM_JOB, 0, 0) };
        }
    });
}

/// Screen position of the text caret in `target`, plus the caret height.
fn caret_anchor(target: HWND) -> (Option<POINT>, i32) {
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    unsafe {
        let thread = wm::GetWindowThreadProcessId(target, std::ptr::null_mut());
        let mut info: wm::GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<wm::GUITHREADINFO>() as u32;
        if thread == 0 || wm::GetGUIThreadInfo(thread, &mut info) == 0 || info.hwndCaret.is_null() {
            return (None, 0);
        }
        let r = info.rcCaret;
        if r.right == 0 && r.bottom == 0 && r.left == 0 && r.top == 0 {
            return (None, 0);
        }
        let mut p = POINT { x: r.left, y: r.top };
        ClientToScreen(info.hwndCaret, &mut p);
        (Some(p), (r.bottom - r.top).max(0))
    }
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_HOTKEY if wparam as i32 == HOTKEY_ID => {
            let start = util::qpc_seconds();
            with(|app| app.show_popup(Opened::Hotkey));
            crate::bench::hotkey_received(start);
            0
        }
        clipboard::WM_CLIP_EVENT => {
            with(App::on_clip_events);
            0
        }
        WM_JOB => {
            with(App::on_jobs);
            0
        }
        tray::WM_TRAY => {
            let event = (lparam & 0xFFFF) as u32;
            match event {
                // Version 4 icons report a click as NIN_SELECT (mouse messages come too; ignore them).
                windows_sys::Win32::UI::Shell::NIN_SELECT | NIN_KEYSELECT => {
                    with(|app| app.show_popup(Opened::Tray));
                }
                wm::WM_CONTEXTMENU => {
                    let (x, y) = lparam_point(wparam as LPARAM);
                    perform(Some(Outside::TrayMenu { x, y }));
                }
                _ => {}
            }
            0
        }
        WM_REMOTE => {
            match Remote::from(wparam) {
                Some(Remote::Show) => {
                    with(|app| app.show_popup(Opened::Command));
                }
                Some(Remote::Settings) => perform(Some(Outside::Settings)),
                Some(Remote::Snippet) => perform(Some(Outside::Snippet)),
                Some(Remote::Quit) => {
                    wm::DestroyWindow(hwnd);
                }
                Some(Remote::Reload) => {
                    with(|app| {
                        let settings = Settings::load(&app.settings_path);
                        app.apply_settings(settings);
                    });
                }
                None => {}
            }
            0
        }
        wm::WM_TIMER => {
            match wparam {
                SAVE_TIMER => {
                    wm::KillTimer(hwnd, SAVE_TIMER);
                    with(|app| app.save_now());
                }
                PRUNE_TIMER => {
                    with(App::prune);
                }
                PASTE_TIMER => {
                    wm::KillTimer(hwnd, PASTE_TIMER);
                    with(App::try_paste);
                }
                _ => {}
            }
            0
        }
        wm::WM_SETTINGCHANGE | wm::WM_SYSCOLORCHANGE => {
            with(App::refresh_theme);
            0
        }
        wm::WM_QUERYENDSESSION => 1,
        wm::WM_ENDSESSION if wparam != 0 => {
            APP.with(|cell| {
                if let Ok(mut guard) = cell.try_borrow_mut() {
                    if let Some(mut app) = guard.take() {
                        app.shutdown();
                    }
                }
            });
            0
        }
        wm::WM_DESTROY => {
            wm::PostQuitMessage(0);
            0
        }
        _ => {
            let recreate = with(|app| msg == app.taskbar_created && msg != 0).unwrap_or(false);
            if recreate {
                with(|app| {
                    let tip = app.tray_tip();
                    let paused = app.settings.paused || !app.settings.history_enabled;
                    app.tray.add(paused, &tip);
                    // A restarted Explorer may have released the shortcut (Win+V after
                    // DisabledHotkeys), so try again.
                    if !app.hotkey_registered {
                        app.register_hotkey();
                        app.redraw_popup();
                    }
                });
                return 0;
            }
            wm::DefWindowProcW(hwnd, msg, wparam, lparam)
        }
    }
}

unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            with(|app| {
                let ctx = Context {
                    history: &app.history,
                    recent: &app.settings.recent,
                    history_enabled: app.settings.history_enabled && !app.policy_blocked,
                    paused: app.settings.paused,
                    policy_blocked: app.policy_blocked,
                    notice: app.notice.as_deref(),
                    now: util::now_ms(),
                };
                app.popup.paint(dc, &ctx);
            });
            EndPaint(hwnd, &ps);
            crate::bench::painted();
            0
        }
        wm::WM_ERASEBKGND => 1,
        // Double-clicking the grip must not maximise the panel.
        wm::WM_NCLBUTTONDBLCLK => 0,
        wm::WM_NCHITTEST => {
            let (sx, sy) = lparam_point(lparam);
            let mut point = POINT { x: sx, y: sy };
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
            if with(|app| app.popup.is_grabber(point.x, point.y)).unwrap_or(false) {
                return wm::HTCAPTION as LRESULT;
            }
            wm::DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        wm::WM_ACTIVATE => {
            if (wparam & 0xFFFF) as u32 == wm::WA_INACTIVE {
                with(|app| {
                    if !app.popup.modal {
                        app.popup.hide();
                    }
                });
            }
            0
        }
        wm::WM_MOUSEACTIVATE => wm::MA_ACTIVATE as LRESULT,
        wm::WM_MOUSEMOVE => {
            let (x, y) = lparam_point(lparam);
            with(|app| {
                let ctx = app.context_static();
                app.popup.on_mouse_move(x, y, &ctx.as_context(&app.history));
            });
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
            0
        }
        windows_sys::Win32::UI::Controls::WM_MOUSELEAVE => {
            with(|app| app.popup.on_mouse_leave());
            0
        }
        wm::WM_LBUTTONDOWN => {
            let (x, y) = lparam_point(lparam);
            with(|app| {
                let ctx = app.context_static();
                app.popup.on_button_down(x, y, &ctx.as_context(&app.history));
            });
            0
        }
        wm::WM_LBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            let outside = with(|app| {
                let ctx = app.context_static();
                let actions = app.popup.on_button_up(x, y, &ctx.as_context(&app.history));
                app.apply_all(actions)
            })
            .flatten();
            perform(outside);
            0
        }
        wm::WM_RBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            let outside = with(|app| {
                let ctx = app.context_static();
                let actions = app.popup.on_right_click(x, y, &ctx.as_context(&app.history));
                app.apply_all(actions)
            })
            .flatten();
            perform(outside);
            0
        }
        wm::WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            with(|app| {
                let ctx = app.context_static();
                app.popup.on_wheel(delta, &ctx.as_context(&app.history));
            });
            0
        }
        wm::WM_KEYDOWN | wm::WM_SYSKEYDOWN => {
            let vk = wparam as u16;
            if msg == wm::WM_SYSKEYDOWN && vk == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_F4 {
                with(|app| app.popup.hide());
                return 0;
            }
            let outside = with(|app| {
                let ctx = app.context_static();
                let actions = app.popup.on_key(vk, &ctx.as_context(&app.history));
                app.apply_all(actions)
            })
            .flatten();
            perform(outside);
            if msg == wm::WM_SYSKEYDOWN {
                return wm::DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            0
        }
        wm::WM_CHAR => {
            if let Some(c) = char::from_u32(wparam as u32) {
                with(|app| {
                    let ctx = app.context_static();
                    app.popup.on_char(c, &ctx.as_context(&app.history));
                });
            }
            0
        }
        wm::WM_TIMER if wparam == TOAST_TIMER => {
            wm::KillTimer(hwnd, TOAST_TIMER);
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        wm::WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            let suggested = *(lparam as *const RECT);
            with(|app| {
                let ctx = app.context_static();
                app.popup.on_dpi_changed(dpi, &suggested, &ctx.as_context(&app.history));
            });
            0
        }
        _ => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The parts of the panel context that do not borrow the history, so the panel can be
/// borrowed mutably at the same time.
pub struct StaticContext {
    recent: Vec<String>,
    history_enabled: bool,
    paused: bool,
    policy_blocked: bool,
    notice: Option<String>,
    now: i64,
}

impl StaticContext {
    fn as_context<'a>(&'a self, history: &'a History) -> Context<'a> {
        Context {
            history,
            recent: &self.recent,
            history_enabled: self.history_enabled,
            paused: self.paused,
            policy_blocked: self.policy_blocked,
            notice: self.notice.as_deref(),
            now: self.now,
        }
    }
}

impl App {
    fn context_static(&self) -> StaticContext {
        StaticContext {
            recent: self.settings.recent.clone(),
            history_enabled: self.settings.history_enabled && !self.policy_blocked,
            paused: self.settings.paused,
            policy_blocked: self.policy_blocked,
            notice: self.notice.clone(),
            now: util::now_ms(),
        }
    }

    fn apply_all(&mut self, actions: Vec<Action>) -> Option<Outside> {
        let mut outside = None;
        for action in actions {
            if let Some(o) = self.apply(action) {
                outside = Some(o);
            }
        }
        outside
    }
}
