//! Small shared helpers: UTF-16 strings, time, folders.

use std::path::PathBuf;
use std::sync::OnceLock;

/// NUL-terminated UTF-16 for Win32 calls.
pub fn wide(text: &str) -> Vec<u16> {
    let mut units = Vec::with_capacity(text.len() + 1);
    units.extend(text.encode_utf16());
    units.push(0);
    units
}

/// Text up to the first NUL (or the whole buffer).
pub fn from_wide(units: &[u16]) -> String {
    let text = units.split(|&unit| unit == 0).next().unwrap_or_default();
    String::from_utf16_lossy(text)
}

/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0usize;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// High resolution timestamp in seconds, for latency measurements.
pub fn qpc_seconds() -> f64 {
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    static FREQUENCY: OnceLock<f64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1) as f64
    });
    let mut counter = 0i64;
    unsafe { QueryPerformanceCounter(&mut counter) };
    counter as f64 / frequency
}

/// Optional profile name, used by tests and benchmarks to run a second, isolated instance.
pub fn profile() -> &'static str {
    static PROFILE: OnceLock<String> = OnceLock::new();
    PROFILE.get_or_init(|| {
        std::env::var("BETTERCLIPBOARD_PROFILE")
            .ok()
            .map(|p| p.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(40).collect())
            .unwrap_or_default()
    })
}

/// %LOCALAPPDATA%\BetterClipboard, the root of everything the app stores.
pub fn data_dir_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("BetterClipboard")
}

/// The data folder of this instance (a profile folder for tests). Clipboard data never
/// leaves the signed-in user's profile.
pub fn data_dir() -> PathBuf {
    let mut dir = data_dir_root();
    if !profile().is_empty() {
        dir = dir.join("profiles").join(profile());
    }
    dir
}

/// "3 min ago" style age, like the Windows clipboard panel does not show but users ask for.
pub fn relative_age(then_ms: i64, now: i64) -> String {
    let seconds = ((now - then_ms) / 1000).max(0);
    match seconds {
        0..=44 => "Just now".into(),
        45..=89 => "1 min ago".into(),
        90..=3599 => format!("{} min ago", (seconds + 30) / 60),
        3600..=7199 => "1 hour ago".into(),
        7200..=86_399 => format!("{} hours ago", seconds / 3600),
        86_400..=172_799 => "Yesterday".into(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// Human readable byte size.
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["bytes", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trip() {
        let text = "Grüße 🌍";
        assert_eq!(from_wide(&wide(text)), text);
    }

    #[test]
    fn ages_read_naturally() {
        let now = 10_000_000_000;
        assert_eq!(relative_age(now - 5_000, now), "Just now");
        assert_eq!(relative_age(now - 60_000, now), "1 min ago");
        assert_eq!(relative_age(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(relative_age(now - 3 * 3_600_000, now), "3 hours ago");
        assert_eq!(relative_age(now - 26 * 3_600_000, now), "Yesterday");
        assert_eq!(relative_age(now + 1_000, now), "Just now");
    }

    #[test]
    fn byte_sizes() {
        assert_eq!(format_bytes(12), "12 bytes");
        assert_eq!(format_bytes(4 * 1024 * 1024), "4.0 MB");
    }
}
