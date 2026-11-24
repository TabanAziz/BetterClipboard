//! Built-in measurements used by docs/benchmarks.md. They run in a separate profile, so
//! they never touch the user's history or settings.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use crate::app::{self, Opened};
use crate::image::Bgra;
use crate::model::{Content, Item, Origin, Picture};
use crate::util;

thread_local! {
    static HOTKEY_AT: Cell<f64> = const { Cell::new(0.0) };
    static HOTKEY_SAMPLES: RefCell<Option<Vec<f64>>> = const { RefCell::new(None) };
}

/// Starts recording hotkey-to-painted latencies (written by [`write_hotkey_log`]).
pub fn record_hotkeys() {
    HOTKEY_SAMPLES.with(|s| *s.borrow_mut() = Some(Vec::new()));
}

pub fn hotkey_received(at: f64) {
    if HOTKEY_SAMPLES.with(|s| s.borrow().is_some()) {
        HOTKEY_AT.with(|h| h.set(at));
    }
}

pub fn painted() {
    let start = HOTKEY_AT.with(|h| h.replace(0.0));
    if start > 0.0 {
        unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
        let elapsed = util::qpc_seconds() - start;
        HOTKEY_SAMPLES.with(|s| {
            if let Some(v) = s.borrow_mut().as_mut() {
                v.push(elapsed * 1000.0);
            }
        });
    }
}

pub fn write_hotkey_log(path: &Path) {
    let samples = HOTKEY_SAMPLES.with(|s| s.borrow_mut().take()).unwrap_or_default();
    let _ = std::fs::write(path, report("hotkey to painted panel (ms)", &samples));
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

pub fn report(title: &str, samples: &[f64]) -> String {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mut out = String::new();
    let _ = writeln!(out, "# {title}");
    let _ = writeln!(
        out,
        "# n={} min={:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2}",
        sorted.len(),
        sorted.first().copied().unwrap_or(0.0),
        percentile(&sorted, 50.0),
        percentile(&sorted, 95.0),
        percentile(&sorted, 99.0),
        sorted.last().copied().unwrap_or(0.0)
    );
    for s in samples {
        let _ = writeln!(out, "{s:.3}");
    }
    out
}

/// Adds `count` synthetic entries: mostly text of varying length, every 25th a picture.
pub fn fill(count: usize) {
    let picture = {
        let mut image = Bgra::new(1920, 1080);
        for (i, p) in image.pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i % 1920, i / 1920);
            p.copy_from_slice(&[(x / 8) as u8, (y / 5) as u8, 128, 255]);
        }
        Picture {
            width: image.width,
            height: image.height,
            png: Arc::new(image.to_png().unwrap_or_default()),
            thumb: Arc::new(image.fit(crate::model::THUMB_W, crate::model::THUMB_H)),
        }
    };
    app::with(|a| {
        a.history.limits.max_items = count.max(25);
        let mut dropped = Vec::new();
        for n in 0..count {
            let content = if n % 25 == 24 {
                Content { picture: Some(picture.clone()), ..Default::default() }
            } else {
                let words = 5 + (n * 7919) % 60;
                let text: String = (0..words).map(|w| format!("word{} ", (n + w) % 97)).collect();
                Content { text: Some(format!("snippet {n}: {text}")), ..Default::default() }
            };
            let id = a.history.allocate_id();
            let item = Item::new(id, content, Some(n as u64), "notepad.exe".into(), Origin::Local, util::now_ms());
            a.history.add(item, &mut dropped);
        }
    });
}

fn wait_for_paint(after: f64) -> f64 {
    let deadline = util::qpc_seconds() + 2.0;
    loop {
        app::pump_pending();
        let painted = app::with(|a| a.popup.painted_at).unwrap_or(0.0);
        if painted > after {
            unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
            return util::qpc_seconds();
        }
        if util::qpc_seconds() > deadline {
            return f64::NAN;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

fn wait_loaded() {
    let deadline = util::qpc_seconds() + 5.0;
    while !app::with(|a| a.loaded).unwrap_or(true) && util::qpc_seconds() < deadline {
        app::pump_pending();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Opens and closes the panel `runs` times with `items` entries and records the time
/// from the open request until the painted frame was handed to the compositor.
pub fn popup(runs: usize, items: usize, path: &Path) {
    wait_loaded();
    fill(items);
    let mut samples = Vec::new();
    for _ in 0..runs {
        let start = util::qpc_seconds();
        app::with(|a| a.show_popup(Opened::Command));
        let end = wait_for_paint(start);
        samples.push((end - start) * 1000.0);
        app::with(|a| a.popup.hide());
        app::pump_pending();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    let _ = std::fs::write(path, report(&format!("open panel with {items} entries until painted (ms)"), &samples));
}

/// Types a query into the panel letter by letter over `items` entries and records the
/// time from the key until the filtered list was painted.
/// Repaints the emoji page again and again (what moving the mouse over it does) and
/// records how long each paint takes, plus the very first paint of the page.
pub fn emoji(path: &Path) {
    wait_loaded();
    app::with(|a| a.show_popup(Opened::Command));
    wait_for_paint(0.0);
    let started = util::qpc_seconds();
    app::with(|a| {
        let ctx = crate::ui::popup::Context {
            history: &a.history,
            recent: &[],
            history_enabled: true,
            paused: false,
            policy_blocked: false,
            notice: None,
            now: util::now_ms(),
        };
        a.popup.open_emoji_page(&ctx);
    });
    wait_for_paint(started);
    let first = app::with(|a| a.popup.paint_ms).unwrap_or(0.0);
    // Measure the colour path, not the outline fallback used while rendering runs.
    let deadline = util::qpc_seconds() + 10.0;
    while !app::with(|a| a.popup.emoji_ready()).unwrap_or(true) && util::qpc_seconds() < deadline {
        app::pump_pending();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let ready_after = (util::qpc_seconds() - started) * 1000.0;
    let mut samples = Vec::new();
    for _ in 0..60 {
        let start = util::qpc_seconds();
        app::with(|a| a.popup.invalidate());
        wait_for_paint(start);
        samples.push(app::with(|a| a.popup.paint_ms).unwrap_or(0.0));
    }
    let mut text = report("emoji page repaint, time inside WM_PAINT (ms)", &samples);
    text.insert_str(
        0,
        &format!(
            "# first paint of the page: {first:.2} ms; colour emoji ready {ready_after:.0} ms after opening it \
             (rendered in the background; at start-up after a 1.5 s delay)\n"
        ),
    );
    let _ = std::fs::write(path, text);
}

pub fn search(items: usize, path: &Path) {
    wait_loaded();
    fill(items);
    app::with(|a| a.show_popup(Opened::Command));
    wait_for_paint(0.0);
    let mut samples = Vec::new();
    for round in 0..4 {
        for c in "snippet 4321 word".chars() {
            let start = util::qpc_seconds();
            app::with(|a| {
                let ctx = crate::ui::popup::Context {
                    history: &a.history,
                    recent: &[],
                    history_enabled: true,
                    paused: false,
                    policy_blocked: false,
                    notice: None,
                    now: util::now_ms(),
                };
                a.popup.on_char(c, &ctx);
            });
            let end = wait_for_paint(start);
            samples.push((end - start) * 1000.0);
        }
        // Clear the query between rounds.
        let _ = round;
        app::with(|a| {
            a.popup.hide();
            a.show_popup(Opened::Command);
        });
        wait_for_paint(0.0);
    }
    let _ = std::fs::write(path, report(&format!("search keystroke until painted, {items} entries (ms)"), &samples));
}

/// Time from process creation until the app is ready (hotkey registered, tray added,
/// message queue idle).
pub fn startup_report(path: &Path) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    app::pump_pending();
    let to_u64 = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    unsafe {
        let mut now: FILETIME = std::mem::zeroed();
        GetSystemTimePreciseAsFileTime(&mut now);
        let (mut created, mut exited, mut kernel, mut user) =
            std::mem::zeroed::<(FILETIME, FILETIME, FILETIME, FILETIME)>();
        GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user);
        let ms = (to_u64(now).saturating_sub(to_u64(created))) as f64 / 10_000.0;
        let _ = std::fs::write(path, format!("{ms:.3}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), 51.0);
        assert_eq!(percentile(&v, 95.0), 95.0);
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
        assert!(report("t", &[1.0, 2.0]).contains("n=2"));
    }
}
