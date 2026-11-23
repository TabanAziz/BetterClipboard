//! Saved history: a versioned binary format, encrypted with DPAPI for the current user.
//!
//! Only the signed-in Windows user can decrypt the file. Writes go to a temporary file
//! first and replace the old one in a single step, so a crash never leaves half a file.

use std::path::Path;
use std::sync::Arc;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

use crate::image::Bgra;
use crate::model::{search_key, Content, Item, Origin, Picture};

const MAGIC: &[u8; 4] = b"BCBH";
const VERSION: u32 = 1;
/// Mixed into the DPAPI key so other programs of the same user cannot decrypt by accident.
const ENTROPY: &[u8] = b"BetterClipboard history v1";

// ------------------------------------------------------------------ DPAPI

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let description = crate::util::wide("BetterClipboard");
    let ok = unsafe {
        CryptProtectData(
            &input,
            description.as_ptr(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("encryption failed ({})", std::io::Error::last_os_error()));
    }
    let out = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as _) };
    Ok(out)
}

pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob(sealed);
    let entropy = blob(ENTROPY);
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("decryption failed ({})", std::io::Error::last_os_error()));
    }
    let out = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as _) };
    Ok(out)
}

// ------------------------------------------------------------------ encoding

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
    fn opt_bytes(&mut self, v: Option<&[u8]>) {
        match v {
            Some(b) => {
                self.u8(1);
                self.bytes(b);
            }
            None => self.u8(0),
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("truncated history file")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| "bad text in history file".to_string())
    }
    fn opt_bytes(&mut self) -> Result<Option<&'a [u8]>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.bytes()?)),
            _ => Err("bad marker in history file".into()),
        }
    }
}

pub fn encode(items: &[Item]) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u32(items.len() as u32);
    for item in items {
        w.u64(item.id);
        w.i64(item.created_ms);
        w.i64(item.last_copied_ms);
        w.u32(item.copies);
        w.u8(item.pinned as u8);
        w.u8(item.origin.code());
        w.u64(item.fingerprint);
        w.bytes(item.app.as_bytes());
        let c = &item.content;
        w.opt_bytes(c.text.as_ref().map(|t| t.as_bytes()));
        w.opt_bytes(c.html.as_deref());
        w.opt_bytes(c.rtf.as_deref());
        w.u32(c.files.len() as u32);
        for f in &c.files {
            w.bytes(f.as_bytes());
        }
        match c.drop_effect {
            Some(e) => {
                w.u8(1);
                w.u32(e);
            }
            None => w.u8(0),
        }
        match &c.picture {
            Some(p) => {
                w.u8(1);
                w.u32(p.width);
                w.u32(p.height);
                w.bytes(&p.png);
                w.u32(p.thumb.width);
                w.u32(p.thumb.height);
                w.bytes(&p.thumb.pixels);
            }
            None => w.u8(0),
        }
    }
    w.0
}

pub fn decode(data: &[u8]) -> Result<Vec<Item>, String> {
    let mut r = Reader { data, at: 0 };
    let count = r.u32()? as usize;
    let mut items = Vec::with_capacity(count.min(100_000));
    for _ in 0..count {
        let id = r.u64()?;
        let created_ms = r.i64()?;
        let last_copied_ms = r.i64()?;
        let copies = r.u32()?;
        let pinned = r.u8()? != 0;
        let origin = Origin::from_code(r.u8()?);
        let fingerprint = r.u64()?;
        let app = r.string()?;
        let text = match r.opt_bytes()? {
            Some(b) => Some(String::from_utf8(b.to_vec()).map_err(|_| "bad text in history file")?),
            None => None,
        };
        let html = r.opt_bytes()?.map(<[u8]>::to_vec);
        let rtf = r.opt_bytes()?.map(<[u8]>::to_vec);
        let file_count = r.u32()? as usize;
        let mut files = Vec::with_capacity(file_count.min(10_000));
        for _ in 0..file_count {
            files.push(r.string()?);
        }
        let drop_effect = if r.u8()? != 0 { Some(r.u32()?) } else { None };
        let picture = if r.u8()? != 0 {
            let width = r.u32()?;
            let height = r.u32()?;
            let png = r.bytes()?.to_vec();
            let tw = r.u32()?;
            let th = r.u32()?;
            let pixels = r.bytes()?.to_vec();
            if pixels.len() != tw as usize * th as usize * 4 {
                return Err("bad thumbnail in history file".into());
            }
            Some(Picture { width, height, png: Arc::new(png), thumb: Arc::new(Bgra { width: tw, height: th, pixels }) })
        } else {
            None
        };
        let content = Content { text, html, rtf, picture, files, drop_effect };
        let key = search_key(&content, &app);
        items.push(Item {
            id,
            content: Arc::new(content),
            search_key: key,
            fingerprint,
            created_ms,
            last_copied_ms,
            copies,
            pinned,
            origin,
            app,
        });
    }
    if r.at != data.len() {
        return Err("unexpected data at the end of the history file".into());
    }
    Ok(items)
}

pub fn seal(items: &[Item]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&protect(&encode(items))?);
    Ok(out)
}

pub fn open(file: &[u8]) -> Result<Vec<Item>, String> {
    if file.len() < 8 || &file[..4] != MAGIC {
        return Err("not a BetterClipboard history file".into());
    }
    let version = u32::from_le_bytes(file[4..8].try_into().unwrap());
    if version != VERSION {
        return Err(format!("history file version {version} is not supported"));
    }
    decode(&unprotect(&file[8..])?)
}

// ------------------------------------------------------------------ files

/// Writes `data` to a temporary file and moves it over `path` in one step.
pub fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Saves the entries (or deletes the file when there is nothing to keep).
pub fn save(path: &Path, items: &[Item]) -> Result<(), String> {
    if items.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    write_atomically(path, &seal(items)?).map_err(|e| e.to_string())
}

pub enum Loaded {
    Missing,
    Items(Vec<Item>),
    /// The file could not be read; it was moved aside (not deleted) under this name.
    Damaged(String),
}

pub fn load(path: &Path) -> Loaded {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Damaged(e.to_string()),
    };
    match open(&data) {
        Ok(items) => Loaded::Items(items),
        Err(reason) => {
            let aside = path.with_extension(format!("damaged-{}", crate::util::now_ms()));
            let _ = std::fs::rename(path, &aside);
            Loaded::Damaged(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Item> {
        let text =
            Content { text: Some("Grüße 🌍".into()), html: Some(b"<b>x</b>".to_vec()), ..Default::default() };
        let thumb = Bgra { width: 1, height: 1, pixels: vec![1, 2, 3, 255] };
        let picture = Content {
            picture: Some(Picture {
                width: 1,
                height: 1,
                png: Arc::new(thumb.to_png().unwrap()),
                thumb: Arc::new(thumb),
            }),
            ..Default::default()
        };
        let files = Content { files: vec![r"C:\a.txt".into()], drop_effect: Some(2), ..Default::default() };
        let mut a = Item::new(1, text, None, "notepad.exe".into(), Origin::Local, 10);
        a.pinned = true;
        let b = Item::new(2, picture, Some(7), "PhoneExperienceHost.exe".into(), Origin::Phone, 20);
        let c = Item::new(3, files, None, "explorer.exe".into(), Origin::Local, 30);
        vec![a, b, c]
    }

    #[test]
    fn encode_decode_round_trip() {
        let items = sample();
        let back = decode(&encode(&items)).unwrap();
        assert_eq!(back.len(), 3);
        for (a, b) in items.iter().zip(&back) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.pinned, b.pinned);
            assert_eq!(a.origin, b.origin);
            assert_eq!(a.fingerprint, b.fingerprint);
            assert_eq!(a.content.text, b.content.text);
            assert_eq!(a.content.html, b.content.html);
            assert_eq!(a.content.files, b.content.files);
            assert_eq!(a.content.drop_effect, b.content.drop_effect);
            assert_eq!(a.search_key, b.search_key);
            assert_eq!(
                a.content.picture.as_ref().map(|p| p.png.clone()),
                b.content.picture.as_ref().map(|p| p.png.clone())
            );
        }
    }

    #[test]
    fn truncated_data_is_rejected_not_panicking() {
        let data = encode(&sample());
        for cut in [0, 1, 4, 10, data.len() / 2, data.len() - 1] {
            assert!(decode(&data[..cut]).is_err(), "cut at {cut}");
        }
        let mut extra = data.clone();
        extra.push(0);
        assert!(decode(&extra).is_err());
    }

    #[test]
    fn sealed_file_is_encrypted() {
        let sealed = seal(&sample()).unwrap();
        let needle = "Grüße".as_bytes();
        assert!(!sealed.windows(needle.len()).any(|w| w == needle), "plain text leaked");
        assert_eq!(open(&sealed).unwrap().len(), 3);
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        assert!(open(&tampered).is_err());
        assert!(open(b"nope").is_err());
    }

    #[test]
    fn save_load_and_damaged_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("bcb-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.bin");
        save(&path, &sample()).unwrap();
        assert!(matches!(load(&path), Loaded::Items(ref v) if v.len() == 3));
        save(&path, &[]).unwrap();
        assert!(!path.exists());
        assert!(matches!(load(&path), Loaded::Missing));
        std::fs::write(&path, b"BCBH\x01\0\0\0garbage").unwrap();
        assert!(matches!(load(&path), Loaded::Damaged(_)));
        assert!(!path.exists());
        let kept = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(kept, 1, "damaged file was moved aside, not deleted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
