//! User settings, stored as a small text file next to the history.
//! The file holds no clipboard content, so it is not encrypted.

use std::path::Path;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    /// MOD_* flags (without MOD_NOREPEAT).
    pub modifiers: u32,
    pub vk: u32,
}

impl Hotkey {
    pub const DEFAULT: Hotkey = Hotkey { modifiers: MOD_WIN | MOD_ALT, vk: b'V' as u32 };
    pub const WIN_V: Hotkey = Hotkey { modifiers: MOD_WIN, vk: b'V' as u32 };

    pub fn register_flags(self) -> u32 {
        self.modifiers | MOD_NOREPEAT
    }

    /// Parses text like "Win+Alt+V", "Ctrl+Shift+Space" or "Ctrl+F9".
    pub fn parse(text: &str) -> Option<Hotkey> {
        let mut modifiers = 0;
        let mut vk = None;
        for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "win" | "windows" | "super" => modifiers |= MOD_WIN,
                "ctrl" | "control" | "strg" => modifiers |= MOD_CONTROL,
                "alt" => modifiers |= MOD_ALT,
                "shift" | "umschalt" => modifiers |= MOD_SHIFT,
                key => {
                    if vk.is_some() {
                        return None;
                    }
                    vk = Some(key_code(key)?);
                }
            }
        }
        // A bare key would swallow normal typing.
        if modifiers == 0 {
            return None;
        }
        Some(Hotkey { modifiers, vk: vk? })
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        for (flag, name) in [(MOD_WIN, "Win"), (MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift")] {
            if self.modifiers & flag != 0 {
                parts.push(name.to_string());
            }
        }
        parts.push(key_name(self.vk));
        parts.join("+")
    }
}

fn key_code(key: &str) -> Option<u32> {
    let upper = key.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Some(c as u32);
        }
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    Some(match upper.as_str() {
        "SPACE" => 0x20,
        "INSERT" | "INS" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "." | "PERIOD" => 0xBE,
        "," | "COMMA" => 0xBC,
        _ => return None,
    })
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x20 => "Space".into(),
        0x2D => "Insert".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0xBE => "Period".into(),
        0xBC => "Comma".into(),
        _ => format!("0x{vk:02X}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    /// Collect clipboard history at all (the Windows "Clipboard history" switch).
    pub history_enabled: bool,
    /// Temporarily stop recording. Kept across restarts so a pause is never lifted silently.
    pub paused: bool,
    pub hotkey: Hotkey,
    /// Unpinned entries kept. 25 matches Windows.
    pub max_items: u32,
    /// Largest entry accepted, in MB. 4 matches Windows (images count compressed).
    pub max_item_mb: u32,
    pub group_duplicates: bool,
    /// Write pinned entries to disk (encrypted). Off means nothing is ever written.
    pub save_pinned: bool,
    /// Also keep unpinned entries across restarts (Windows clears them).
    pub keep_history: bool,
    /// Remove unpinned entries older than this many days (0 = never).
    pub retention_days: u32,
    /// Paste into the previous window after choosing an entry (off: only copy).
    pub paste_on_select: bool,
    pub theme: Theme,
    /// Clipboard owners whose content is never recorded (process file names).
    pub excluded_apps: Vec<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            history_enabled: true,
            paused: false,
            hotkey: Hotkey::DEFAULT,
            max_items: 25,
            max_item_mb: 4,
            group_duplicates: true,
            save_pinned: true,
            keep_history: false,
            retention_days: 0,
            paste_on_select: true,
            theme: Theme::System,
            excluded_apps: vec![
                "KeePass.exe".into(),
                "KeePassXC.exe".into(),
                "1Password.exe".into(),
                "Bitwarden.exe".into(),
            ],
        }
    }
}

pub const MAX_ITEMS_LIMIT: u32 = 100_000;
pub const MAX_ITEM_MB_LIMIT: u32 = 256;

impl Settings {
    pub fn is_excluded(&self, exe: &str) -> bool {
        !exe.is_empty() && self.excluded_apps.iter().any(|e| e.eq_ignore_ascii_case(exe))
    }

    pub fn max_item_bytes(&self) -> usize {
        self.max_item_mb.clamp(1, MAX_ITEM_MB_LIMIT) as usize * 1024 * 1024
    }

    pub fn to_text(&self) -> String {
        let flag = |b: bool| if b { "1" } else { "0" };
        let theme = match self.theme {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        };
        let mut out = String::from("# BetterClipboard settings\n");
        out += &format!("history_enabled={}\n", flag(self.history_enabled));
        out += &format!("paused={}\n", flag(self.paused));
        out += &format!("hotkey={}\n", self.hotkey.label());
        out += &format!("max_items={}\n", self.max_items);
        out += &format!("max_item_mb={}\n", self.max_item_mb);
        out += &format!("group_duplicates={}\n", flag(self.group_duplicates));
        out += &format!("save_pinned={}\n", flag(self.save_pinned));
        out += &format!("keep_history={}\n", flag(self.keep_history));
        out += &format!("retention_days={}\n", self.retention_days);
        out += &format!("paste_on_select={}\n", flag(self.paste_on_select));
        out += &format!("theme={theme}\n");
        out += &format!("excluded_apps={}\n", self.excluded_apps.join(";"));
        out
    }

    /// Unknown keys and bad values are ignored; the default stays.
    pub fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            let flag = value == "1" || value.eq_ignore_ascii_case("true");
            let number = value.parse::<u32>().ok();
            match key.trim() {
                "history_enabled" => s.history_enabled = flag,
                "paused" => s.paused = flag,
                "hotkey" => s.hotkey = Hotkey::parse(value).unwrap_or(s.hotkey),
                "max_items" => s.max_items = number.unwrap_or(s.max_items).clamp(1, MAX_ITEMS_LIMIT),
                "max_item_mb" => s.max_item_mb = number.unwrap_or(s.max_item_mb).clamp(1, MAX_ITEM_MB_LIMIT),
                "group_duplicates" => s.group_duplicates = flag,
                "save_pinned" => s.save_pinned = flag,
                "keep_history" => s.keep_history = flag,
                "retention_days" => s.retention_days = number.unwrap_or(0).min(3650),
                "paste_on_select" => s.paste_on_select = flag,
                "theme" => {
                    s.theme = match value {
                        "light" => Theme::Light,
                        "dark" => Theme::Dark,
                        _ => Theme::System,
                    }
                }
                "excluded_apps" => {
                    s.excluded_apps =
                        value.split(';').map(str::trim).filter(|v| !v.is_empty()).map(String::from).collect()
                }
                _ => {}
            }
        }
        s
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::store::write_atomically(path, self.to_text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_parse_and_print() {
        assert_eq!(Hotkey::parse("Win+Alt+V"), Some(Hotkey::DEFAULT));
        assert_eq!(Hotkey::parse(" ctrl + shift + space ").unwrap().label(), "Ctrl+Shift+Space");
        assert_eq!(Hotkey::parse("Ctrl+F9").unwrap().vk, 0x78);
        assert_eq!(Hotkey::parse("V"), None, "needs a modifier");
        assert_eq!(Hotkey::parse("Ctrl+A+B"), None);
        assert_eq!(Hotkey::parse("Ctrl+Nope"), None);
        assert_eq!(Hotkey::WIN_V.label(), "Win+V");
    }

    #[test]
    fn settings_round_trip() {
        let s = Settings {
            max_items: 500,
            keep_history: true,
            theme: Theme::Dark,
            excluded_apps: vec!["a.exe".into(), "b.exe".into()],
            hotkey: Hotkey::parse("Ctrl+Shift+F1").unwrap(),
            ..Default::default()
        };
        assert_eq!(Settings::from_text(&s.to_text()), s);
    }

    #[test]
    fn bad_values_fall_back() {
        let s = Settings::from_text("max_items=abc\nmax_item_mb=99999\nhotkey=Q\nunknown=1\nnot a line");
        assert_eq!(s.max_items, 25);
        assert_eq!(s.max_item_mb, MAX_ITEM_MB_LIMIT);
        assert_eq!(s.hotkey, Hotkey::DEFAULT);
    }

    #[test]
    fn exclusions_ignore_case() {
        let s = Settings::default();
        assert!(s.is_excluded("keepass.EXE"));
        assert!(!s.is_excluded("notepad.exe"));
        assert!(!s.is_excluded(""));
    }
}
