//! Integration tests against the real Windows clipboard.
//!
//! They replace whatever is on the clipboard, so they are ignored by default:
//!     cargo test --test system_clipboard -- --ignored --test-threads=1

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bcb::clipboard::{CaptureConfig, Command, Event, Service, Skip};
use bcb::image::Bgra;
use bcb::model::{Content, Picture};
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

static SERIAL: Mutex<()> = Mutex::new(());

fn ui_window() -> HWND {
    let class = bcb::util::wide("STATIC");
    unsafe {
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
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    }
}

fn config() -> CaptureConfig {
    CaptureConfig { enabled: true, max_bytes: 4 * 1024 * 1024, excluded: Vec::new() }
}

fn format(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(bcb::util::wide(name).as_ptr()) }
}

/// Puts formats on the clipboard the way another program would.
fn put(parts: &[(u32, Vec<u8>)]) {
    for _ in 0..50 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        EmptyClipboard();
        for (fmt, bytes) in parts {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
            let p = GlobalLock(h) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            GlobalUnlock(h);
            if SetClipboardData(*fmt, h as HANDLE).is_null() {
                GlobalFree(h);
            }
        }
        CloseClipboard();
    }
}

fn get(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(fmt);
        let out = if h.is_null() {
            None
        } else {
            let size = GlobalSize(h as _);
            let p = GlobalLock(h as _) as *const u8;
            let v = std::slice::from_raw_parts(p, size).to_vec();
            GlobalUnlock(h as _);
            Some(v)
        };
        CloseClipboard();
        out
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    let mut v: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    v.extend_from_slice(&[0, 0]);
    v
}

fn pump_until(events: &Receiver<Event>, timeout: Duration) -> Option<Event> {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        unsafe {
            let mut msg: wm::MSG = std::mem::zeroed();
            while wm::PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, wm::PM_REMOVE) != 0 {
                wm::DispatchMessageW(&msg);
            }
        }
        if let Ok(e) = events.try_recv() {
            return Some(e);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn captured(events: &Receiver<Event>) -> bcb::clipboard::Captured {
    match pump_until(events, Duration::from_secs(5)) {
        Some(Event::Captured(c)) => *c,
        Some(Event::Skipped(s)) => panic!("skipped: {s:?}"),
        Some(Event::Written { .. }) => panic!("unexpected write result"),
        None => panic!("no clipboard event"),
    }
}

fn sample_image() -> Bgra {
    let mut image = Bgra::new(64, 32);
    for (i, p) in image.pixels.chunks_exact_mut(4).enumerate() {
        p.copy_from_slice(&[(i % 64 * 4) as u8, (i / 64 * 8) as u8, 77, 255]);
    }
    image
}

#[test]
#[ignore = "uses the system clipboard"]
fn captures_text_html_images_and_files() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (service, events) = Service::start(ui_window(), config());
    assert!(service.is_running());

    // C01: plain Unicode text.
    put(&[(CF_UNICODETEXT as u32, utf16z("Grüße 🌍 BetterClipboard"))]);
    let c = captured(&events);
    assert_eq!(c.content.text.as_deref(), Some("Grüße 🌍 BetterClipboard"));

    // Rich text: HTML and RTF are kept next to the text.
    let html = b"Version:0.9\r\nStartHTML:0\r\nEndHTML:0\r\nStartFragment:0\r\nEndFragment:0\r\n<b>bold</b>".to_vec();
    let rtf = br"{\rtf1\ansi \b bold\b0}".to_vec();
    put(&[
        (CF_UNICODETEXT as u32, utf16z("bold")),
        (format("HTML Format"), html.clone()),
        (format("Rich Text Format"), rtf.clone()),
    ]);
    let c = captured(&events);
    assert_eq!(c.content.html.as_deref(), Some(&html[..]));
    assert_eq!(c.content.rtf.as_deref(), Some(&rtf[..]));

    // Screenshot-style bitmap (CF_DIB).
    let image = sample_image();
    put(&[(CF_DIB as u32, image.to_dib())]);
    let c = captured(&events);
    let picture = c.content.picture.expect("picture");
    assert_eq!((picture.width, picture.height), (64, 32));
    assert_eq!(Bgra::from_png(&picture.png).unwrap(), image);

    // Browsers offer "PNG" with transparency.
    let mut transparent = sample_image();
    transparent.pixels[3] = 0;
    put(&[(format("PNG"), transparent.to_png().unwrap())]);
    let c = captured(&events);
    let picture = c.content.picture.expect("png picture");
    assert_eq!(Bgra::from_png(&picture.png).unwrap(), transparent);

    // Files copied in Explorer (CF_HDROP + Preferred DropEffect for "cut").
    let files = vec![r"C:\Windows\win.ini".to_string(), r"C:\Windows\notepad.exe".to_string()];
    put(&[
        (CF_HDROP as u32, bcb::clipfmt::build_hdrop(&files)),
        (format("Preferred DropEffect"), 2u32.to_le_bytes().to_vec()),
    ]);
    let c = captured(&events);
    assert_eq!(c.content.files, files);
    assert_eq!(c.content.drop_effect, Some(2));
}

#[test]
#[ignore = "uses the system clipboard"]
fn respects_privacy_markers_limits_and_pause() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (service, events) = Service::start(ui_window(), config());

    // Password managers mark copies as private.
    put(&[
        (CF_UNICODETEXT as u32, utf16z("secret")),
        (format("ExcludeClipboardContentFromMonitorProcessing"), vec![0]),
    ]);
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Skipped(Skip::AppAskedNotToRecord))));

    put(&[
        (CF_UNICODETEXT as u32, utf16z("secret 2")),
        (format("CanIncludeInClipboardHistory"), 0u32.to_le_bytes().to_vec()),
    ]);
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Skipped(Skip::AppAskedNotToRecord))));

    put(&[
        (CF_UNICODETEXT as u32, utf16z("allowed")),
        (format("CanIncludeInClipboardHistory"), 1u32.to_le_bytes().to_vec()),
    ]);
    assert_eq!(captured(&events).content.text.as_deref(), Some("allowed"));

    // C04: the 4 MB limit (text counts as UTF-16).
    let big = "x".repeat(3 * 1024 * 1024);
    put(&[(CF_UNICODETEXT as u32, utf16z(&big))]);
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Skipped(Skip::TooLarge(_)))));

    // A full-screen 4K screenshot (33 MB as a raw bitmap) is still recorded.
    let mut screen = Bgra::new(3840, 2160);
    for (i, p) in screen.pixels.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % 3840, i / 3840);
        p.copy_from_slice(&[(x / 15) as u8, (y / 9) as u8, ((x ^ y) & 0x1F) as u8, 255]);
    }
    put(&[(CF_DIB as u32, screen.to_dib())]);
    // Unoptimised test builds need a while to compress 8 million pixels.
    let picture = match pump_until(&events, Duration::from_secs(60)) {
        Some(Event::Captured(c)) => c.content.picture.expect("4K screenshot"),
        _ => panic!("4K screenshot was not recorded"),
    };
    assert_eq!((picture.width, picture.height), (3840, 2160));

    // Empty and broken formats are ignored, not crashed on.
    put(&[(format("BetterClipboard.Test.Private"), vec![1, 2, 3])]);
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Skipped(Skip::Unsupported))));
    put(&[(CF_DIB as u32, vec![40, 0, 0, 0, 1])]);
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Skipped(Skip::Unsupported))));

    // C09: paused recording records nothing.
    service.send(Command::Configure(CaptureConfig { enabled: false, ..config() }));
    std::thread::sleep(Duration::from_millis(100));
    put(&[(CF_UNICODETEXT as u32, utf16z("while paused"))]);
    assert!(pump_until(&events, Duration::from_millis(600)).is_none());
}

#[test]
#[ignore = "uses the system clipboard"]
fn writes_entries_back_in_all_formats() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (service, events) = Service::start(ui_window(), config());
    let image = sample_image();
    let content = Content {
        text: Some("Grüße".into()),
        html: Some(b"<i>Gr&uuml;&szlig;e</i>".to_vec()),
        picture: Some(Picture {
            width: 64,
            height: 32,
            png: Arc::new(image.to_png().unwrap()),
            thumb: Arc::new(image.fit(16, 16)),
        }),
        ..Default::default()
    };
    service.send(Command::Write {
        id: 7,
        content: Arc::new(content),
        plain_text: false,
        paste: false,
        keep_local: false,
        fingerprint: 0,
    });
    match pump_until(&events, Duration::from_secs(5)) {
        Some(Event::Written { id: 7, paste: false, result: Ok(()) }) => {}
        _ => panic!("write failed"),
    }
    // Our own write must not come back as a new entry.
    assert!(pump_until(&events, Duration::from_millis(500)).is_none());

    let text = get(CF_UNICODETEXT as u32).unwrap();
    let units: Vec<u16> = text.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(bcb::util::from_wide(&units), "Grüße");
    assert_eq!(get(format("HTML Format")).unwrap(), b"<i>Gr&uuml;&szlig;e</i>");
    assert_eq!(bcb::image::decode_dib(&get(CF_DIB as u32).unwrap()).unwrap(), image);
    assert_eq!(Bgra::from_png(&get(format("PNG")).unwrap()).unwrap(), image);

    // Plain text mode drops the formatting.
    let rich = Content { text: Some("plain".into()), html: Some(b"<b>plain</b>".to_vec()), ..Default::default() };
    service.send(Command::Write {
        id: 8,
        content: Arc::new(rich),
        plain_text: true,
        paste: false,
        keep_local: false,
        fingerprint: 0,
    });
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Written { id: 8, result: Ok(()), .. })));
    assert!(get(format("HTML Format")).is_none());

    // Items from the phone are marked so Phone Link and cloud sync do not send them back.
    let phone = Content { text: Some("from the phone".into()), ..Default::default() };
    service.send(Command::Write {
        id: 10,
        content: Arc::new(phone),
        plain_text: false,
        paste: false,
        keep_local: true,
        fingerprint: 0,
    });
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Written { id: 10, result: Ok(()), .. })));
    assert!(get(format("ExcludeClipboardContentFromMonitorProcessing")).is_some());
    assert_eq!(get(format("CanUploadToCloudClipboard")).unwrap()[..4], 0u32.to_le_bytes());
    assert_eq!(get(format("CanIncludeInClipboardHistory")).unwrap()[..4], 0u32.to_le_bytes());

    // Choosing the phone item that is still on the clipboard writes nothing (no new
    // clipboard change for Phone Link to send back to the phone).
    put(&[(CF_UNICODETEXT as u32, utf16z("copied on the phone"))]);
    let captured = match pump_until(&events, Duration::from_secs(5)) {
        Some(Event::Captured(c)) => c,
        _ => panic!("phone text not captured"),
    };
    let print = captured.content.fingerprint(captured.image_print);
    let before = unsafe { GetClipboardSequenceNumber() };
    let same = Content { text: Some("copied on the phone".into()), ..Default::default() };
    service.send(Command::Write {
        id: 11,
        content: Arc::new(same),
        plain_text: false,
        paste: false,
        keep_local: true,
        fingerprint: print,
    });
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Written { id: 11, result: Ok(()), .. })));
    assert_eq!(unsafe { GetClipboardSequenceNumber() }, before, "clipboard was rewritten");

    // Files go back as CF_HDROP.
    let files = Content { files: vec![r"C:\Windows\win.ini".into()], drop_effect: Some(5), ..Default::default() };
    service.send(Command::Write {
        id: 9,
        content: Arc::new(files),
        plain_text: false,
        paste: false,
        keep_local: false,
        fingerprint: 0,
    });
    assert!(matches!(pump_until(&events, Duration::from_secs(5)), Some(Event::Written { id: 9, result: Ok(()), .. })));
    assert_eq!(bcb::clipfmt::parse_hdrop(&get(CF_HDROP as u32).unwrap()), vec![r"C:\Windows\win.ini".to_string()]);
}

#[test]
#[ignore = "uses the system clipboard"]
fn high_copy_rate_and_locked_clipboard() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_service, events) = Service::start(ui_window(), config());
    // C04/C06: many copies in a row; every one is either captured or coalesced, none crash.
    for n in 0..200 {
        put(&[(CF_UNICODETEXT as u32, utf16z(&format!("burst {n}")))]);
    }
    let mut last = String::new();
    while let Some(Event::Captured(c)) = pump_until(&events, Duration::from_millis(800)) {
        last = c.content.text.unwrap_or_default();
    }
    assert_eq!(last, "burst 199", "the newest copy is always recorded");

    // The clipboard held open by another program for a while: capture retries.
    put(&[(CF_UNICODETEXT as u32, utf16z("after lock"))]);
    unsafe { OpenClipboard(std::ptr::null_mut()) };
    std::thread::sleep(Duration::from_millis(250));
    unsafe { CloseClipboard() };
    let mut got = None;
    while let Some(e) = pump_until(&events, Duration::from_secs(3)) {
        if let Event::Captured(c) = e {
            got = c.content.text;
            break;
        }
    }
    assert_eq!(got.as_deref(), Some("after lock"));
}
