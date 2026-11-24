//! Colour emoji. GDI draws emoji only as outlines, so emoji are drawn with DirectWrite
//! through a Direct2D render target bound to the panel's back buffer.

use windows::core::PCWSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;

/// Text and its rectangle (x, y, width, height) in pixels.
pub type EmojiCell<'a> = (&'a str, (i32, i32, i32, i32));

pub struct EmojiRenderer {
    target: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory,
    format: Option<(i32, IDWriteTextFormat)>,
}

impl EmojiRenderer {
    pub fn new() -> Option<EmojiRenderer> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = factory.CreateDCRenderTarget(&properties).ok()?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            Some(EmojiRenderer { target, dwrite, format: None })
        }
    }

    fn format(&mut self, size_px: i32) -> Option<IDWriteTextFormat> {
        if let Some((size, format)) = &self.format {
            if *size == size_px {
                return Some(format.clone());
            }
        }
        let family: Vec<u16> = "Segoe UI Emoji".encode_utf16().chain([0]).collect();
        let locale: Vec<u16> = "en-us".encode_utf16().chain([0]).collect();
        let format = unsafe {
            let format = self
                .dwrite
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
            format
        };
        self.format = Some((size_px, format.clone()));
        Some(format)
    }

    /// Draws each text centred in its rectangle, in colour, clipped to `clip`.
    /// `hdc` is the panel's memory DC; `size` its pixel size.
    pub fn draw(
        &mut self,
        hdc: *mut core::ffi::c_void,
        size: (i32, i32),
        clip: (i32, i32, i32, i32),
        cells: &[EmojiCell],
        size_px: i32,
        fallback: u32,
    ) -> bool {
        if cells.is_empty() {
            return true;
        }
        let Some(format) = self.format(size_px) else { return false };
        unsafe {
            let whole = RECT { left: 0, top: 0, right: size.0, bottom: size.1 };
            if self.target.BindDC(HDC(hdc), &whole).is_err() {
                return false;
            }
            let color = D2D1_COLOR_F {
                r: ((fallback >> 16) & 0xFF) as f32 / 255.0,
                g: ((fallback >> 8) & 0xFF) as f32 / 255.0,
                b: (fallback & 0xFF) as f32 / 255.0,
                a: 1.0,
            };
            let Ok(brush): windows::core::Result<ID2D1SolidColorBrush> =
                self.target.CreateSolidColorBrush(&color, None)
            else {
                return false;
            };
            self.target.BeginDraw();
            let (cx, cy, cw, ch) = clip;
            self.target.PushAxisAlignedClip(
                &D2D_RECT_F { left: cx as f32, top: cy as f32, right: (cx + cw) as f32, bottom: (cy + ch) as f32 },
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
            );
            for (text, (x, y, w, h)) in cells {
                let units: Vec<u16> = text.encode_utf16().collect();
                let rect =
                    D2D_RECT_F { left: *x as f32, top: *y as f32, right: (x + w) as f32, bottom: (y + h) as f32 };
                self.target.DrawText(
                    &units,
                    &format,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT | D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            self.target.PopAxisAlignedClip();
            self.target.EndDraw(None, None).is_ok()
        }
    }
}
