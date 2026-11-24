//! Colour emoji. GDI draws emoji only as outlines, so they are rendered with DirectWrite
//! through Direct2D. Rendering colour glyphs is slow (starting Direct2D and loading the
//! emoji font alone takes a few hundred milliseconds), so it happens once per size on a
//! background thread. The panel only copies the finished bitmaps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};

/// One rendered emoji: premultiplied BGRA pixels, `size` x `size`.
pub struct Sprite {
    pub size: i32,
    pub pixels: Vec<u32>,
}

pub type Sprites = Arc<HashMap<&'static str, Sprite>>;

enum Slot {
    Rendering,
    Ready(Sprites),
    Failed,
}

fn cache() -> &'static Mutex<HashMap<i32, Slot>> {
    static CACHE: OnceLock<Mutex<HashMap<i32, Slot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The rendered emoji for this pixel size, if they are ready.
pub fn sprites(size_px: i32) -> Option<Sprites> {
    match cache().lock().ok()?.get(&size_px) {
        Some(Slot::Ready(sprites)) => Some(Arc::clone(sprites)),
        _ => None,
    }
}

/// Starts rendering all emoji at this size on a background thread, once, after
/// `delay_ms`. When done, `notify` runs on that thread (the panel asks for a repaint).
pub fn prepare(size_px: i32, delay_ms: u64, notify: impl FnOnce() + Send + 'static) {
    {
        let Ok(mut cache) = cache().lock() else { return };
        if cache.contains_key(&size_px) {
            return;
        }
        cache.insert(size_px, Slot::Rendering);
    }
    let spawned = std::thread::Builder::new().name("emoji".into()).spawn(move || {
        // Below normal priority: this must never compete with the panel or start-up.
        unsafe {
            windows_sys::Win32::System::Threading::SetThreadPriority(
                windows_sys::Win32::System::Threading::GetCurrentThread(),
                windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL,
            )
        };
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let texts: Vec<&'static str> = crate::symbols::EMOJI.iter().map(|(e, _)| *e).collect();
        let slot = match render(&texts, size_px) {
            Some(sprites) => Slot::Ready(Arc::new(sprites)),
            None => Slot::Failed,
        };
        if let Ok(mut cache) = cache().lock() {
            cache.insert(size_px, slot);
        }
        notify();
    });
    if spawned.is_err() {
        if let Ok(mut cache) = cache().lock() {
            cache.insert(size_px, Slot::Failed);
        }
    }
}

/// Edge length of the bitmap that holds one emoji drawn at `size_px`.
pub fn sprite_size(size_px: i32) -> i32 {
    size_px * 3 / 2
}

/// Renders every text into its own premultiplied bitmap. Runs on a worker thread with
/// its own Direct2D and DirectWrite objects.
fn render(texts: &[&'static str], size_px: i32) -> Option<HashMap<&'static str, Sprite>> {
    let cell = sprite_size(size_px);
    let width = cell * texts.len() as i32;
    unsafe {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = factory.CreateDCRenderTarget(&properties).ok()?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let family: Vec<u16> = "Segoe UI Emoji".encode_utf16().chain([0]).collect();
        let locale: Vec<u16> = "en-us".encode_utf16().chain([0]).collect();
        let format = dwrite
            .CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_px as f32,
                PCWSTR(locale.as_ptr()),
            )
            .ok()?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok()?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok()?;

        // One strip with all emoji side by side, drawn in a single pass.
        let dc = CreateCompatibleDC(None);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -cell;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap): windows::core::Result<HBITMAP> =
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, bitmap.into());
        let result = (|| {
            let rect = RECT { left: 0, top: 0, right: width, bottom: cell };
            target.BindDC(dc, &rect).ok()?;
            let black = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
            let brush = target.CreateSolidColorBrush(&black, None).ok()?;
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            for (i, text) in texts.iter().enumerate() {
                let units: Vec<u16> = text.encode_utf16().collect();
                let x = (i as i32 * cell) as f32;
                let layout = D2D_RECT_F { left: x, top: 0.0, right: x + cell as f32, bottom: cell as f32 };
                target.DrawText(
                    &units,
                    &format,
                    &layout,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            target.EndDraw(None, None).ok()?;
            let _ = GdiFlush();
            let strip = std::slice::from_raw_parts(bits as *const u32, (width * cell) as usize);
            let mut sprites = HashMap::with_capacity(texts.len());
            for (i, text) in texts.iter().enumerate() {
                let mut pixels = Vec::with_capacity((cell * cell) as usize);
                for row in 0..cell {
                    let start = (row * width + i as i32 * cell) as usize;
                    pixels.extend_from_slice(&strip[start..start + cell as usize]);
                }
                sprites.insert(*text, Sprite { size: cell, pixels });
            }
            Some(sprites)
        })();
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_colour_emoji_in_the_background() {
        let (tx, rx) = std::sync::mpsc::channel();
        prepare(24, 0, move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(20)).expect("emoji rendering finished");
        let sprites = sprites(24).expect("sprites ready");
        assert_eq!(sprites.len(), crate::symbols::EMOJI.len());
        let grin = &sprites["😀"];
        assert_eq!(grin.size, sprite_size(24));
        // A colour emoji has pixels that are neither transparent nor grey.
        let coloured = grin.pixels.iter().any(|&p| {
            let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            p >> 24 > 0 && (r.abs_diff(b) > 40 || g.abs_diff(b) > 40)
        });
        assert!(coloured, "emoji were not rendered in colour");
    }
}
