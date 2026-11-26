//! The clipboard worker thread.
//!
//! It owns a message-only window registered with `AddClipboardFormatListener`, so Windows
//! tells it about every change (no polling). Reading and writing clipboard data happens
//! only here: a slow or hung clipboard owner (delayed rendering) can stall this thread,
//! never the UI.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::clipfmt;
use crate::image::{self, Bgra};
use crate::model::{self, Content, Origin, Picture};
use crate::util::wide;

/// Posted to the UI window whenever an [`Event`] is queued.
pub const WM_CLIP_EVENT: u32 = wm::WM_APP + 1;
const WM_COMMAND_QUEUED: u32 = wm::WM_APP + 2;
const RETRY_TIMER: usize = 1;
/// Compressed size up to which pictures are always kept.
pub const MIN_PICTURE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub enabled: bool,
    pub max_bytes: usize,
    pub excluded: Vec<String>,
}

pub struct Captured {
    pub content: Content,
    pub image_print: Option<u64>,
    pub app: String,
    pub origin: Origin,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    /// The source app marked the data as private (password managers do this).
    AppAskedNotToRecord,
    Excluded(String),
    TooLarge(usize),
    Locked,
    Unsupported,
}

pub enum Event {
    Captured(Box<Captured>),
    Skipped(Skip),
    /// Result of a [`Command::Write`].
    Written {
        id: u64,
        paste: bool,
        result: Result<(), String>,
    },
}

pub enum Command {
    Write {
        id: u64,
        content: Arc<Content>,
        plain_text: bool,
        paste: bool,
        /// Keep the data on this PC: mark it so clipboard monitors (Phone Link's
        /// cross-device copy, Windows history and cloud sync) do not pass it on. Used for
        /// items that came from the phone, so they are not sent back to it.
        keep_local: bool,
        /// Fingerprint of the item. With `keep_local`, nothing is written when the
        /// clipboard still holds exactly this item: rewriting it would make Phone Link send
        /// it to the phone again.
        fingerprint: u64,
    },
    Configure(CaptureConfig),
    /// Empties the system clipboard (Settings > Clear clipboard data).
    Clear,
    Quit,
}

pub struct Service {
    commands: Sender<Command>,
    window: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Service {
    pub fn start(ui: HWND, config: CaptureConfig) -> (Service, Receiver<Event>) {
        let (event_tx, event_rx) = channel();
        let (command_tx, command_rx) = channel();
        let (ready_tx, ready_rx) = channel();
        let ui = ui as isize;
        let thread = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || worker(ui, config, event_tx, command_rx, ready_tx))
            .expect("clipboard thread");
        let window = ready_rx.recv().unwrap_or(0);
        (Service { commands: command_tx, window, thread: Some(thread) }, event_rx)
    }

    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            unsafe { wm::PostMessageW(self.window as HWND, WM_COMMAND_QUEUED, 0, 0) };
        }
    }

    pub fn is_running(&self) -> bool {
        self.window != 0
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Formats {
    html: u32,
    rtf: u32,
    png: u32,
    drop_effect: u32,
    exclude: u32,
    can_include: u32,
    can_upload: u32,
    viewer_ignore: u32,
}

impl Formats {
    fn register() -> Formats {
        let reg = |name: &str| unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) };
        Formats {
            html: reg("HTML Format"),
            rtf: reg("Rich Text Format"),
            png: reg("PNG"),
            drop_effect: reg("Preferred DropEffect"),
            exclude: reg("ExcludeClipboardContentFromMonitorProcessing"),
            can_include: reg("CanIncludeInClipboardHistory"),
            can_upload: reg("CanUploadToCloudClipboard"),
            viewer_ignore: reg("Clipboard Viewer Ignore"),
        }
    }
}

struct Worker {
    ui: isize,
    window: HWND,
    config: CaptureConfig,
    events: Sender<Event>,
    commands: Receiver<Command>,
    formats: Formats,
    handled_sequence: u32,
    own_sequence: u32,
    retries: u32,
    /// Sequence number and fingerprint of the last recorded clipboard content.
    current: Option<(u32, u64)>,
}

thread_local! {
    static WORKER: std::cell::RefCell<Option<Worker>> = const { std::cell::RefCell::new(None) };
}

fn worker(ui: isize, config: CaptureConfig, events: Sender<Event>, commands: Receiver<Command>, ready: Sender<isize>) {
    let class = wide("BetterClipboard.Listener");
    let window = unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let mut definition: wm::WNDCLASSEXW = std::mem::zeroed();
        definition.cbSize = std::mem::size_of_val(&definition) as u32;
        definition.hInstance = module;
        definition.lpszClassName = class.as_ptr();
        definition.lpfnWndProc = Some(listener_proc);
        wm::RegisterClassExW(&definition);
        wm::CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            wm::HWND_MESSAGE,
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        let _ = ready.send(0);
        return;
    }
    let listening = unsafe { AddClipboardFormatListener(window) } != 0;
    let sequence = unsafe { GetClipboardSequenceNumber() };
    WORKER.with(|w| {
        *w.borrow_mut() = Some(Worker {
            ui,
            window,
            config,
            events,
            commands,
            formats: Formats::register(),
            // What is on the clipboard at start-up was copied before; Windows does not add it either.
            handled_sequence: sequence,
            own_sequence: 0,
            retries: 0,
            current: None,
        })
    });
    let _ = ready.send(if listening { window as isize } else { 0 });

    unsafe {
        let mut message: wm::MSG = std::mem::zeroed();
        while wm::GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            wm::DispatchMessageW(&message);
        }
        RemoveClipboardFormatListener(window);
        wm::DestroyWindow(window);
    }
    WORKER.with(|w| w.borrow_mut().take());
}

unsafe extern "system" fn listener_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        wm::WM_CLIPBOARDUPDATE => {
            WORKER.with(|w| {
                if let Some(worker) = w.borrow_mut().as_mut() {
                    worker.retries = 0;
                    worker.on_update();
                }
            });
            0
        }
        wm::WM_TIMER if wparam == RETRY_TIMER => {
            wm::KillTimer(hwnd, RETRY_TIMER);
            WORKER.with(|w| {
                if let Some(worker) = w.borrow_mut().as_mut() {
                    worker.on_update();
                }
            });
            0
        }
        WM_COMMAND_QUEUED => {
            let quit = WORKER.with(|w| w.borrow_mut().as_mut().map(Worker::drain_commands).unwrap_or(true));
            if quit {
                wm::PostQuitMessage(0);
            }
            0
        }
        _ => wm::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Opens the clipboard, retrying briefly while another program holds it.
fn open_clipboard(window: HWND) -> bool {
    for attempt in 0..8u64 {
        if unsafe { OpenClipboard(window) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5 + attempt * 5));
    }
    false
}

struct OpenGuard;
impl Drop for OpenGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

/// Copies a global-memory clipboard format, refusing anything above `limit` bytes.
unsafe fn read_global(format: u32, limit: usize) -> Result<Option<Vec<u8>>, usize> {
    if IsClipboardFormatAvailable(format) == 0 {
        return Ok(None);
    }
    let handle = GetClipboardData(format);
    if handle.is_null() {
        return Ok(None);
    }
    let size = GlobalSize(handle as _);
    if size > limit {
        return Err(size);
    }
    let data = GlobalLock(handle as _) as *const u8;
    if data.is_null() {
        return Ok(None);
    }
    let bytes = std::slice::from_raw_parts(data, size).to_vec();
    GlobalUnlock(handle as _);
    Ok(Some(bytes))
}

/// File name of the process that owns a window, e.g. "notepad.exe".
pub fn window_process_name(window: HWND) -> String {
    if window.is_null() {
        return String::new();
    }
    let mut pid = 0u32;
    unsafe { wm::GetWindowThreadProcessId(window, &mut pid) };
    process_name(pid)
}

pub fn process_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length);
        windows_sys::Win32::Foundation::CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_string()
    }
}

impl Worker {
    fn emit(&self, event: Event) {
        if self.events.send(event).is_ok() {
            unsafe { wm::PostMessageW(self.ui as HWND, WM_CLIP_EVENT, 0, 0) };
        }
    }

    /// Returns true when the thread should stop.
    fn drain_commands(&mut self) -> bool {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Quit => return true,
                Command::Configure(config) => self.config = config,
                Command::Clear => {
                    if open_clipboard(self.window) {
                        let emptied = {
                            let _guard = OpenGuard;
                            unsafe { EmptyClipboard() != 0 }
                        };
                        if emptied {
                            self.remember_own_write();
                        }
                    }
                }
                Command::Write { id, content, plain_text, paste, keep_local, fingerprint } => {
                    let unchanged = self.current == Some((unsafe { GetClipboardSequenceNumber() }, fingerprint));
                    let screenshot = content.picture.is_some()
                        && content.text.is_none()
                        && content.html.is_none()
                        && content.rtf.is_none()
                        && content.files.is_empty();
                    let result = if unchanged && !plain_text && (keep_local || screenshot) {
                        Ok(())
                    } else {
                        self.write(&content, plain_text, keep_local)
                    };
                    self.emit(Event::Written { id, paste, result });
                }
            }
        }
        false
    }

    fn remember_own_write(&mut self) {
        // Closing the clipboard can advance the sequence. Sample the committed state,
        // and do not mark a newer copy if another application has taken ownership.
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if unsafe { GetClipboardOwner() } == self.window {
            self.own_sequence = sequence;
        }
    }

    fn on_update(&mut self) {
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if sequence == self.handled_sequence {
            return;
        }
        if sequence == self.own_sequence {
            // Our own write (an entry chosen from the history); the UI already moved it to the top.
            self.handled_sequence = sequence;
            return;
        }
        if !self.config.enabled {
            self.handled_sequence = sequence;
            return;
        }
        let f = &self.formats;
        let private =
            unsafe { IsClipboardFormatAvailable(f.exclude) != 0 || IsClipboardFormatAvailable(f.viewer_ignore) != 0 };
        if private {
            self.handled_sequence = sequence;
            self.emit(Event::Skipped(Skip::AppAskedNotToRecord));
            return;
        }

        let owner = unsafe { GetClipboardOwner() };
        let mut app = window_process_name(owner);
        if app.is_empty() {
            // Some programs put data on the clipboard without an owner window; the active
            // window is the best remaining guess.
            app = window_process_name(unsafe { wm::GetForegroundWindow() });
        }
        if self.config.excluded.iter().any(|e| e.eq_ignore_ascii_case(&app)) {
            self.handled_sequence = sequence;
            self.emit(Event::Skipped(Skip::Excluded(app)));
            return;
        }

        if !open_clipboard(self.window) {
            // Somebody keeps the clipboard open; try again shortly, a few times.
            if self.retries < 5 {
                self.retries += 1;
                unsafe { wm::SetTimer(self.window, RETRY_TIMER, 60 * self.retries, None) };
            } else {
                self.handled_sequence = sequence;
                self.emit(Event::Skipped(Skip::Locked));
            }
            return;
        }
        let (mut sequence, read) = {
            let _guard = OpenGuard;
            if unsafe { GetClipboardSequenceNumber() } != sequence {
                unsafe { wm::PostMessageW(self.window, wm::WM_CLIPBOARDUPDATE, 0, 0) };
                return;
            }
            let read = self.read_open_clipboard();
            // Reading a delayed format can advance the sequence while the clipboard is open.
            (unsafe { GetClipboardSequenceNumber() }, read)
        };
        let captured = read.and_then(|raw| self.finish(raw, app, &mut sequence));
        self.handled_sequence = sequence;
        match captured {
            Ok(captured) => {
                self.current = Some((sequence, captured.content.fingerprint(captured.image_print)));
                self.emit(Event::Captured(Box::new(captured)))
            }
            Err(skip) => self.emit(Event::Skipped(skip)),
        }
    }

    fn read_open_clipboard(&self) -> Result<Raw, Skip> {
        let f = &self.formats;
        let limit = self.config.max_bytes;
        let too_large = Skip::TooLarge;
        unsafe {
            if IsClipboardFormatAvailable(f.can_include) != 0 {
                if let Ok(Some(flag)) = read_global(f.can_include, 64) {
                    if flag.len() >= 4 && u32::from_le_bytes(flag[..4].try_into().unwrap()) == 0 {
                        return Err(Skip::AppAskedNotToRecord);
                    }
                }
            }
            let text = read_global(CF_UNICODETEXT as u32, limit).map_err(too_large)?.map(|b| {
                let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                crate::util::from_wide(&units)
            });
            let html = read_global(f.html, limit).map_err(too_large)?;
            let rtf = read_global(f.rtf, limit).map_err(too_large)?;
            let files = read_global(CF_HDROP as u32, limit).map_err(too_large)?.map(|b| clipfmt::parse_hdrop(&b));
            let drop_effect = read_global(f.drop_effect, 64)
                .ok()
                .flatten()
                .filter(|b| b.len() >= 4)
                .map(|b| u32::from_le_bytes(b[..4].try_into().unwrap()));
            let picture = self.read_picture(0)?;
            Ok(Raw { text, html, rtf, files: files.unwrap_or_default(), drop_effect, picture })
        }
    }

    /// Respect the producer's format order. A screenshot can publish a ready bitmap and
    /// offer PNG as a slower secondary format rendered while the clipboard is locked.
    unsafe fn read_picture(&self, after: u32) -> Result<Option<RawPicture>, Skip> {
        let limit = self.config.max_bytes;
        let png_limit = limit.max(MIN_PICTURE_BYTES);
        // Images are checked after compression, so allow larger raw bitmaps.
        let dib_limit = (image::MAX_PIXELS as usize * 4).min(limit.saturating_mul(64)).max(limit);
        let mut too_large = None;
        let mut format = EnumClipboardFormats(after);
        while format != 0 {
            let candidate = if format == self.formats.png {
                read_global(format, png_limit)
            } else if format == CF_DIBV5 as u32 || format == CF_DIB as u32 {
                read_global(format, dib_limit)
            } else {
                Ok(None)
            };
            match candidate {
                Ok(Some(bytes)) => return Ok(Some(RawPicture { format, bytes })),
                Ok(None) => {}
                Err(size) => {
                    too_large.get_or_insert(size);
                }
            }
            format = EnumClipboardFormats(format);
        }
        too_large.map_or(Ok(None), |size| Err(Skip::TooLarge(size)))
    }

    fn finish_picture(
        &self,
        mut candidate: Option<RawPicture>,
        sequence: &mut u32,
    ) -> Result<Option<(Picture, u64)>, Skip> {
        let mut too_large = None;
        while let Some(RawPicture { format, bytes }) = candidate {
            let decoded = if format == self.formats.png {
                Bgra::from_png(&bytes).map(|bgra| (bgra, Some(bytes)))
            } else {
                image::decode_dib(&bytes).map(|bgra| (bgra, None))
            };
            if let Ok((bgra, png)) = decoded {
                let fingerprint = bgra.fingerprint();
                let picture = picture_from(bgra, png);
                if picture.png.len() <= self.config.max_bytes.max(MIN_PICTURE_BYTES) {
                    return Ok(Some((picture, fingerprint)));
                }
                too_large = Some(picture.png.len());
            }

            // Only request a fallback after a failed decode, and only from the same copy.
            if !open_clipboard(self.window) {
                break;
            }
            let _guard = OpenGuard;
            if unsafe { GetClipboardSequenceNumber() } != *sequence {
                break;
            }
            let read = unsafe { self.read_picture(format) };
            *sequence = unsafe { GetClipboardSequenceNumber() };
            candidate = read?;
        }
        too_large.map_or(Ok(None), |size| Err(Skip::TooLarge(size)))
    }

    /// Turns raw formats into history content; image work happens after the clipboard is closed.
    fn finish(&self, raw: Raw, app: String, sequence: &mut u32) -> Result<Captured, Skip> {
        let (picture, image_print) = self
            .finish_picture(raw.picture, sequence)?
            .map_or((None, None), |(picture, fingerprint)| (Some(picture), Some(fingerprint)));
        let content = Content {
            text: raw.text.filter(|t| !t.is_empty()),
            html: raw.html,
            rtf: raw.rtf,
            picture,
            files: raw.files,
            drop_effect: raw.drop_effect,
        };
        if content.is_empty() {
            return Err(Skip::Unsupported);
        }
        // Pictures are counted compressed and get a larger allowance, so full-screen
        // screenshots of 4K and 5K displays still show up.
        let picture_bytes = content.picture.as_ref().map_or(0, |p| p.png.len());
        let size = content.size();
        if size - picture_bytes > self.config.max_bytes {
            return Err(Skip::TooLarge(size));
        }
        let origin = model::origin_for_process(&app);
        Ok(Captured { content, image_print, app, origin })
    }

    fn write(&mut self, content: &Content, plain_text: bool, keep_local: bool) -> Result<(), String> {
        let f = &self.formats;
        let mut parts: Vec<(u32, Vec<u8>)> = Vec::new();
        let text =
            if content.text.is_some() || !plain_text { content.text.clone() } else { Some(content.preview_text()) };
        let text = text.filter(|t| !plain_text || !t.trim().is_empty());
        if let Some(text) = text {
            let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.extend_from_slice(&[0, 0]);
            parts.push((CF_UNICODETEXT as u32, bytes));
        }
        if !plain_text {
            if let Some(html) = &content.html {
                parts.push((f.html, html.clone()));
            }
            if let Some(rtf) = &content.rtf {
                parts.push((f.rtf, rtf.clone()));
            }
            if !content.files.is_empty() {
                parts.push((CF_HDROP as u32, clipfmt::build_hdrop(&content.files)));
                if let Some(effect) = content.drop_effect {
                    parts.push((f.drop_effect, effect.to_le_bytes().to_vec()));
                }
            }
            if let Some(picture) = &content.picture {
                let bgra = Bgra::from_png(&picture.png).map_err(|e| format!("The image could not be read: {e}"))?;
                parts.push((CF_DIB as u32, bgra.to_dib()));
                parts.push((f.png, picture.png.to_vec()));
            }
        }
        if parts.is_empty() {
            return Err("There is nothing to paste as plain text.".into());
        }
        if keep_local {
            parts.push((f.exclude, vec![0]));
            parts.push((f.can_include, 0u32.to_le_bytes().to_vec()));
            parts.push((f.can_upload, 0u32.to_le_bytes().to_vec()));
        }
        if !open_clipboard(self.window) {
            return Err("Another app is using the clipboard. Try again.".into());
        }
        let mut failed = false;
        {
            let _guard = OpenGuard;
            unsafe {
                EmptyClipboard();
                for (format, bytes) in &parts {
                    if !set_global(*format, bytes) {
                        failed = true;
                    }
                }
            }
        }
        self.remember_own_write();
        if failed {
            Err("Windows did not accept all clipboard formats.".into())
        } else {
            Ok(())
        }
    }
}

struct Raw {
    text: Option<String>,
    html: Option<Vec<u8>>,
    rtf: Option<Vec<u8>>,
    files: Vec<String>,
    drop_effect: Option<u32>,
    picture: Option<RawPicture>,
}

struct RawPicture {
    format: u32,
    bytes: Vec<u8>,
}

fn picture_from(bgra: Bgra, png: Option<Vec<u8>>) -> Picture {
    let thumb = bgra.fit(model::THUMB_W, model::THUMB_H);
    let png = match png {
        Some(p) => p,
        None => bgra.to_png().unwrap_or_default(),
    };
    Picture { width: bgra.width, height: bgra.height, png: Arc::new(png), thumb: Arc::new(thumb) }
}

unsafe fn set_global(format: u32, bytes: &[u8]) -> bool {
    let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
    if handle.is_null() {
        return false;
    }
    let target = GlobalLock(handle) as *mut u8;
    if target.is_null() {
        GlobalFree(handle);
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
    GlobalUnlock(handle);
    if SetClipboardData(format, handle as HANDLE).is_null() {
        GlobalFree(handle);
        return false;
    }
    true
}
