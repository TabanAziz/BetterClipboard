//! Clipboard history data model. Pure logic, no Win32 calls.

use std::sync::Arc;

use crate::clipfmt;
use crate::image::Bgra;

/// Where an entry came from, as far as the clipboard owner tells us.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Origin {
    #[default]
    Local,
    /// Copied on a phone and handed over by Phone Link (cross-device copy and paste).
    Phone,
    /// Created by hand in BetterClipboard.
    Snippet,
}

impl Origin {
    pub fn code(self) -> u8 {
        match self {
            Origin::Local => 0,
            Origin::Phone => 1,
            Origin::Snippet => 2,
        }
    }
    pub fn from_code(code: u8) -> Origin {
        match code {
            1 => Origin::Phone,
            2 => Origin::Snippet,
            _ => Origin::Local,
        }
    }
}

/// Process names Phone Link uses when it writes phone clipboard content on the PC.
pub const PHONE_PROCESSES: &[&str] = &[
    "phoneexperiencehost.exe",
    "yourphone.exe",
    "yourphoneserver.exe",
    "phonelink.exe",
    "crossdeviceservice.exe",
    "crossdeviceresume.exe",
];

pub fn origin_for_process(exe: &str) -> Origin {
    let lower = exe.to_ascii_lowercase();
    if PHONE_PROCESSES.contains(&lower.as_str()) {
        Origin::Phone
    } else {
        Origin::Local
    }
}

/// Friendly app name for a process file name.
pub fn app_label(exe: &str) -> String {
    let lower = exe.to_ascii_lowercase();
    let known = [
        ("phoneexperiencehost.exe", "Phone Link"),
        ("yourphone.exe", "Phone Link"),
        ("yourphoneserver.exe", "Phone Link"),
        ("phonelink.exe", "Phone Link"),
        ("crossdeviceservice.exe", "Phone Link"),
        ("crossdeviceresume.exe", "Phone Link"),
        ("screenclippinghost.exe", "Snipping Tool"),
        ("snippingtool.exe", "Snipping Tool"),
        ("msedge.exe", "Microsoft Edge"),
        ("chrome.exe", "Google Chrome"),
        ("firefox.exe", "Firefox"),
        ("explorer.exe", "File Explorer"),
        ("notepad.exe", "Notepad"),
        ("winword.exe", "Word"),
        ("excel.exe", "Excel"),
        ("powerpnt.exe", "PowerPoint"),
        ("outlook.exe", "Outlook"),
        ("olk.exe", "Outlook"),
        ("code.exe", "Visual Studio Code"),
        ("powershell.exe", "Windows PowerShell"),
        ("pwsh.exe", "PowerShell"),
        ("cmd.exe", "Command Prompt"),
        ("windowsterminal.exe", "Terminal"),
        ("mspaint.exe", "Paint"),
        ("acrobat.exe", "Acrobat"),
        ("acrord32.exe", "Acrobat Reader"),
        ("betterclipboard.exe", "BetterClipboard"),
    ];
    if let Some((_, name)) = known.iter().find(|(file, _)| *file == lower) {
        return (*name).to_string();
    }
    let stem = exe.rsplit_once('.').map_or(exe, |(stem, _)| stem);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Full image, PNG encoded (keeps memory low; decoded when pasted or saved).
    pub png: Arc<Vec<u8>>,
    /// Small preview for the list, at most THUMB_W x THUMB_H.
    pub thumb: Arc<Bgra>,
}

pub const THUMB_W: u32 = 680;
pub const THUMB_H: u32 = 280;

/// Everything captured from one clipboard update.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub text: Option<String>,
    pub html: Option<Vec<u8>>,
    pub rtf: Option<Vec<u8>>,
    pub picture: Option<Picture>,
    pub files: Vec<String>,
    /// "Preferred DropEffect" for files (2 = move, i.e. cut in Explorer).
    pub drop_effect: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Link,
    Rich,
    Image,
    Files,
}

impl Content {
    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty)
            && self.html.is_none()
            && self.rtf.is_none()
            && self.picture.is_none()
            && self.files.is_empty()
    }

    pub fn kind(&self) -> Kind {
        if !self.files.is_empty() {
            Kind::Files
        } else if self.picture.is_some() && self.text.as_deref().is_none_or(|t| t.trim().is_empty()) {
            Kind::Image
        } else if self.html.is_some() || self.rtf.is_some() {
            Kind::Rich
        } else if self.text.as_deref().is_some_and(clipfmt::looks_like_link) {
            Kind::Link
        } else if self.text.is_some() {
            Kind::Text
        } else {
            Kind::Image
        }
    }

    /// Bytes the entry occupies, used for the per-entry size limit.
    pub fn size(&self) -> usize {
        self.text.as_ref().map_or(0, |t| t.len() * 2)
            + self.html.as_ref().map_or(0, Vec::len)
            + self.rtf.as_ref().map_or(0, Vec::len)
            + self.picture.as_ref().map_or(0, |p| p.png.len())
            + self.files.iter().map(|f| f.len() * 2).sum::<usize>()
    }

    /// Text shown in the list (falls back to the HTML/RTF text or file names).
    pub fn preview_text(&self) -> String {
        if let Some(text) = &self.text {
            if !text.trim().is_empty() {
                return text.clone();
            }
        }
        if !self.files.is_empty() {
            return self.files.join("\n");
        }
        if let Some(html) = &self.html {
            return clipfmt::html_to_text(&clipfmt::html_fragment(html));
        }
        if let Some(rtf) = &self.rtf {
            return clipfmt::rtf_to_text(rtf);
        }
        String::new()
    }

    /// Stable fingerprint of the content, for grouping duplicates.
    pub fn fingerprint(&self, image_print: Option<u64>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut hasher);
        // HTML headers carry offsets and URLs that change between copies; hash the fragment.
        self.html.as_ref().map(|h| clipfmt::html_fragment(h)).hash(&mut hasher);
        self.files.hash(&mut hasher);
        image_print.hash(&mut hasher);
        if self.text.is_none() && self.html.is_none() {
            self.rtf.hash(&mut hasher);
        }
        hasher.finish()
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub content: Arc<Content>,
    /// Lower-case text used by search, cut to a bounded length.
    pub search_key: Arc<str>,
    pub fingerprint: u64,
    pub created_ms: i64,
    pub last_copied_ms: i64,
    pub copies: u32,
    pub pinned: bool,
    pub origin: Origin,
    /// Process file name of the clipboard owner, e.g. "notepad.exe".
    pub app: String,
}

/// Longest text (in chars) that search looks at per entry.
pub const SEARCH_CHARS: usize = 16 * 1024;

pub fn search_key(content: &Content, app: &str) -> Arc<str> {
    let mut key: String = content.preview_text().chars().take(SEARCH_CHARS).collect::<String>().to_lowercase();
    key.push('\u{1}');
    key.push_str(&app_label(app).to_lowercase());
    key.into()
}

impl Item {
    pub fn new(id: u64, content: Content, image_print: Option<u64>, app: String, origin: Origin, now: i64) -> Item {
        let fingerprint = content.fingerprint(image_print);
        let search_key = search_key(&content, &app);
        Item {
            id,
            content: Arc::new(content),
            search_key,
            fingerprint,
            created_ms: now,
            last_copied_ms: now,
            copies: 1,
            pinned: origin == Origin::Snippet,
            origin,
            app,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Filter {
    #[default]
    All,
    Text,
    Images,
    Files,
    Phone,
    Pinned,
}

impl Filter {
    pub const ALL: [Filter; 6] =
        [Filter::All, Filter::Text, Filter::Images, Filter::Files, Filter::Phone, Filter::Pinned];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Text => "Text",
            Filter::Images => "Images",
            Filter::Files => "Files",
            Filter::Phone => "Phone",
            Filter::Pinned => "Pinned",
        }
    }

    pub fn accepts(self, item: &Item) -> bool {
        match self {
            Filter::All => true,
            Filter::Text => matches!(item.content.kind(), Kind::Text | Kind::Link | Kind::Rich),
            Filter::Images => item.content.picture.is_some(),
            Filter::Files => !item.content.files.is_empty(),
            Filter::Phone => item.origin == Origin::Phone,
            Filter::Pinned => item.pinned,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Unpinned entries kept (Windows keeps 25).
    pub max_items: usize,
    pub group_duplicates: bool,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_items: 25, group_duplicates: true }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    New(u64),
    /// An identical entry existed and moved to the top.
    Merged(u64),
}

/// The history list, newest first.
#[derive(Default)]
pub struct History {
    items: Vec<Item>,
    next_id: u64,
    pub limits: Limits,
    /// Bumped on every change, so views know when to rebuild.
    pub revision: u64,
}

impl History {
    pub fn new(limits: Limits) -> History {
        History { items: Vec::new(), next_id: 1, limits, revision: 0 }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Adds a captured entry at the top. Returns the ids of entries dropped by the limit
    /// through `dropped`.
    pub fn add(&mut self, mut item: Item, dropped: &mut Vec<u64>) -> Added {
        self.touch();
        // Some programs announce one copy twice (OLE flush); that is not a second copy.
        if let Some(top) = self.items.first() {
            if top.fingerprint == item.fingerprint && (item.last_copied_ms - top.last_copied_ms).abs() < 1500 {
                return Added::Merged(top.id);
            }
        }
        if self.limits.group_duplicates {
            if let Some(pos) = self.items.iter().position(|i| i.fingerprint == item.fingerprint) {
                let mut existing = self.items.remove(pos);
                existing.copies = existing.copies.saturating_add(1);
                existing.last_copied_ms = item.last_copied_ms;
                if existing.origin == Origin::Local && item.origin == Origin::Phone {
                    existing.origin = Origin::Phone;
                }
                let id = existing.id;
                self.items.insert(0, existing);
                return Added::Merged(id);
            }
        }
        if item.id == 0 || self.items.iter().any(|i| i.id == item.id) {
            item.id = self.allocate_id();
        } else {
            self.next_id = self.next_id.max(item.id + 1);
        }
        let id = item.id;
        self.items.insert(0, item);
        self.enforce_limit(dropped);
        Added::New(id)
    }

    /// Appends at the bottom (used when loading saved entries, oldest last).
    pub fn push_loaded(&mut self, item: Item) {
        self.next_id = self.next_id.max(item.id + 1);
        self.items.push(item);
        self.touch();
    }

    /// Drops the oldest unpinned entries beyond the limit. Pinned entries never count.
    pub fn enforce_limit(&mut self, dropped: &mut Vec<u64>) {
        let mut unpinned = 0usize;
        let max = self.limits.max_items.max(1);
        let before = self.items.len();
        self.items.retain(|i| {
            if i.pinned {
                return true;
            }
            unpinned += 1;
            if unpinned > max {
                dropped.push(i.id);
                false
            } else {
                true
            }
        });
        if self.items.len() != before {
            self.touch();
        }
    }

    /// Moves an entry to the top (it was pasted or copied again).
    pub fn promote(&mut self, id: u64, now: i64) -> bool {
        let Some(pos) = self.items.iter().position(|i| i.id == id) else { return false };
        let mut item = self.items.remove(pos);
        item.last_copied_ms = now;
        self.items.insert(0, item);
        self.touch();
        true
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        let removed = self.items.len() != before;
        if removed {
            self.touch();
        }
        removed
    }

    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> bool {
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { return false };
        item.pinned = pinned;
        self.touch();
        let mut dropped = Vec::new();
        self.enforce_limit(&mut dropped);
        true
    }

    /// "Clear all": removes everything except pinned entries. Returns removed ids.
    pub fn clear_unpinned(&mut self) -> Vec<u64> {
        let removed: Vec<u64> = self.items.iter().filter(|i| !i.pinned).map(|i| i.id).collect();
        self.items.retain(|i| i.pinned);
        self.touch();
        removed
    }

    /// Removes unpinned entries last copied before `cutoff_ms`.
    pub fn prune_older_than(&mut self, cutoff_ms: i64) -> Vec<u64> {
        let removed: Vec<u64> =
            self.items.iter().filter(|i| !i.pinned && i.last_copied_ms < cutoff_ms).map(|i| i.id).collect();
        if !removed.is_empty() {
            self.items.retain(|i| i.pinned || i.last_copied_ms >= cutoff_ms);
            self.touch();
        }
        removed
    }

    /// Indexes of entries matching the filter and every word of the query.
    pub fn search(&self, query: &str, filter: Filter) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| filter.accepts(item))
            .filter(|(_, item)| words.iter().all(|w| item.search_key.contains(w.as_str())))
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Content {
        Content { text: Some(s.into()), ..Default::default() }
    }

    fn item(history: &mut History, s: &str) -> Item {
        let id = history.allocate_id();
        Item::new(id, text(s), None, "notepad.exe".into(), Origin::Local, 1000)
    }

    #[test]
    fn newest_first_and_limit_keeps_pinned() {
        let mut h = History::new(Limits { max_items: 3, group_duplicates: true });
        let mut dropped = Vec::new();
        for s in ["a", "b", "c"] {
            let i = item(&mut h, s);
            h.add(i, &mut dropped);
        }
        let first_id = h.items()[2].id;
        assert!(h.set_pinned(first_id, true));
        for s in ["d", "e"] {
            let i = item(&mut h, s);
            h.add(i, &mut dropped);
        }
        let texts: Vec<_> = h.items().iter().map(|i| i.content.text.clone().unwrap()).collect();
        assert_eq!(texts, ["e", "d", "c", "a"]);
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn duplicates_move_to_top() {
        let mut h = History::new(Limits::default());
        let mut dropped = Vec::new();
        let a = item(&mut h, "same");
        let a_id = a.id;
        h.add(a, &mut dropped);
        let b = item(&mut h, "other");
        h.add(b, &mut dropped);
        let again = item(&mut h, "same");
        assert_eq!(h.add(again, &mut dropped), Added::Merged(a_id));
        assert_eq!(h.items()[0].id, a_id);
        assert_eq!(h.items()[0].copies, 2);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn duplicates_kept_when_grouping_is_off() {
        let mut h = History::new(Limits { max_items: 25, group_duplicates: false });
        let mut dropped = Vec::new();
        for n in 0..3 {
            let mut i = item(&mut h, "same");
            i.last_copied_ms += n * 10_000;
            h.add(i, &mut dropped);
        }
        assert_eq!(h.len(), 3);
        let ids: std::collections::HashSet<u64> = h.items().iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn an_immediate_echo_is_not_a_second_copy() {
        for group in [true, false] {
            let mut h = History::new(Limits { max_items: 25, group_duplicates: group });
            let mut dropped = Vec::new();
            let first = item(&mut h, "same");
            h.add(first, &mut dropped);
            let echo = item(&mut h, "same");
            h.add(echo, &mut dropped);
            assert_eq!((h.len(), h.items()[0].copies), (1, 1));
            let mut later = item(&mut h, "same");
            later.last_copied_ms += 10_000;
            h.add(later, &mut dropped);
            assert_eq!(h.len(), if group { 1 } else { 2 });
        }
    }

    #[test]
    fn clear_all_keeps_pinned_and_delete_removes() {
        let mut h = History::new(Limits::default());
        let mut dropped = Vec::new();
        for s in ["a", "b", "c"] {
            let i = item(&mut h, s);
            h.add(i, &mut dropped);
        }
        let pinned = h.items()[1].id;
        h.set_pinned(pinned, true);
        let removed = h.clear_unpinned();
        assert_eq!(removed.len(), 2);
        assert_eq!(h.len(), 1);
        assert!(h.remove(pinned));
        assert!(h.is_empty());
        assert!(!h.remove(pinned));
    }

    #[test]
    fn promote_and_prune() {
        let mut h = History::new(Limits::default());
        let mut dropped = Vec::new();
        for s in ["old", "new"] {
            let i = item(&mut h, s);
            h.add(i, &mut dropped);
        }
        let old = h.items()[1].id;
        assert!(h.promote(old, 5000));
        assert_eq!(h.items()[0].id, old);
        let removed = h.prune_older_than(2000);
        assert_eq!(removed.len(), 1);
        assert_eq!(h.items()[0].id, old);
    }

    #[test]
    fn search_matches_all_words_case_insensitively() {
        let mut h = History::new(Limits::default());
        let mut dropped = Vec::new();
        for s in ["Hello World", "hello there", "Grüße aus Wien"] {
            let i = item(&mut h, s);
            h.add(i, &mut dropped);
        }
        assert_eq!(h.search("HELLO", Filter::All).len(), 2);
        assert_eq!(h.search("hello world", Filter::All).len(), 1);
        assert_eq!(h.search("GRÜSSE", Filter::All).len(), 0);
        assert_eq!(h.search("grüße", Filter::All).len(), 1);
        assert_eq!(h.search("notepad", Filter::All).len(), 3, "app name is searchable");
        assert_eq!(h.search("", Filter::Pinned).len(), 0);
    }

    #[test]
    fn phone_items_are_recognised() {
        assert_eq!(origin_for_process("PhoneExperienceHost.exe"), Origin::Phone);
        assert_eq!(origin_for_process("notepad.exe"), Origin::Local);
        assert_eq!(app_label("PhoneExperienceHost.exe"), "Phone Link");
        assert_eq!(app_label("someTool.exe"), "SomeTool");
        let mut h = History::new(Limits::default());
        let mut dropped = Vec::new();
        let id = h.allocate_id();
        let phone = Item::new(id, text("from phone"), None, "PhoneExperienceHost.exe".into(), Origin::Phone, 1);
        h.add(phone, &mut dropped);
        let local = item(&mut h, "local");
        h.add(local, &mut dropped);
        assert_eq!(h.search("", Filter::Phone).len(), 1);
        assert_eq!(h.search("phone link", Filter::All).len(), 1);
    }

    #[test]
    fn kinds() {
        assert_eq!(text("https://example.com").kind(), Kind::Link);
        assert_eq!(text("plain").kind(), Kind::Text);
        let files = Content { files: vec!["C:\\a".into()], ..Default::default() };
        assert_eq!(files.kind(), Kind::Files);
        let rich = Content { text: Some("x".into()), html: Some(b"<b>x</b>".to_vec()), ..Default::default() };
        assert_eq!(rich.kind(), Kind::Rich);
        assert!(Content::default().is_empty());
        assert!(text("").is_empty());
    }

    #[test]
    fn html_header_changes_do_not_break_grouping() {
        let a = Content {
            text: Some("x".into()),
            html: Some(b"StartFragment:0000000010\r\nX<b>x</b>".to_vec()),
            ..Default::default()
        };
        let b = Content {
            text: Some("x".into()),
            html: Some(b"StartFragment:0000000010\r\nX<b>x</b>".to_vec()),
            ..Default::default()
        };
        assert_eq!(a.fingerprint(None), b.fingerprint(None));
    }

    #[test]
    fn ten_thousand_entries_search_fast() {
        let mut h = History::new(Limits { max_items: 20_000, group_duplicates: false });
        let mut dropped = Vec::new();
        for n in 0..10_000 {
            let i = item(&mut h, &format!("snippet number {n} with some ordinary text to look through"));
            h.add(i, &mut dropped);
        }
        let start = std::time::Instant::now();
        let hits = h.search("number 9999", Filter::All);
        let elapsed = start.elapsed();
        assert_eq!(hits.len(), 1);
        // Generous bound so slow CI machines pass; docs/benchmarks.md has measured numbers.
        assert!(elapsed.as_millis() < 200, "search took {elapsed:?}");
    }
}
