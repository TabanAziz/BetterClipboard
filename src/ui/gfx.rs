//! Drawing layer, shared with Better Task Manager: one 32-bit DIB back buffer per window.
//! GDI draws text into it; anti-aliased rounded rectangles and pictures are rasterised
//! directly into the pixel memory. Pixels are 0xAARRGGBB.

use windows_sys::Win32::Foundation::{RECT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;

use crate::image::Bgra;
use crate::util::wide;

pub type Color = u32; // 0xRRGGBB

#[inline]
pub fn cr(c: Color) -> u32 {
    // 0xRRGGBB -> COLORREF 0x00BBGGRR
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

#[inline]
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| -> u32 {
        let x = ((a >> s) & 0xFF) as f32;
        let y = ((b >> s) & 0xFF) as f32;
        ((x + (y - x) * t).round() as u32) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

pub struct Canvas {
    pub hdc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    px: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    pub fn new() -> Canvas {
        let hdc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        unsafe {
            SetBkMode(hdc, TRANSPARENT as i32);
        }
        Canvas { hdc, bmp: std::ptr::null_mut(), old: std::ptr::null_mut(), px: std::ptr::null_mut(), w: 0, h: 0 }
    }

    pub fn ensure(&mut self, w: i32, h: i32) {
        let w = w.max(1);
        let h = h.max(1);
        if w == self.w && h == self.h && !self.bmp.is_null() {
            return;
        }
        unsafe {
            if !self.bmp.is_null() {
                SelectObject(self.hdc, self.old);
                DeleteObject(self.bmp);
            }
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h;
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            self.bmp = CreateDIBSection(self.hdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            self.px = bits as *mut u32;
            self.old = SelectObject(self.hdc, self.bmp);
        }
        self.w = w;
        self.h = h;
    }

    #[inline]
    fn flush(&self) {
        unsafe { GdiFlush() };
    }

    #[inline]
    fn clip(&self, x: i32, y: i32, w: i32, h: i32) -> Option<(i32, i32, i32, i32)> {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w);
        let y1 = (y + h).min(self.h);
        if x1 <= x0 || y1 <= y0 || self.px.is_null() {
            None
        } else {
            Some((x0, y0, x1, y1))
        }
    }

    fn row(&mut self, y: i32, x0: i32, x1: i32) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.px.add((y * self.w + x0) as usize), (x1 - x0) as usize) }
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        self.flush();
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        let v = 0xFF00_0000 | c;
        for yy in y0..y1 {
            self.row(yy, x0, x1).fill(v);
        }
    }

    #[inline]
    fn blend_px(&mut self, x: i32, y: i32, c: Color, a: f32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let p = unsafe { &mut *self.px.add((y * self.w + x) as usize) };
        *p = 0xFF00_0000 | mix(*p & 0xFF_FFFF, c, a);
    }

    pub fn blend(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, a: f32) {
        self.flush();
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.blend_px(xx, yy, c, a);
            }
        }
    }

    fn coverage(x: f32, y: f32, w: f32, h: f32, r: f32, xx: i32, yy: i32) -> f32 {
        let (hx, hy) = (x + w / 2.0, y + h / 2.0);
        let px = xx as f32 + 0.5;
        let py = yy as f32 + 0.5;
        let cx = (px - hx).abs() - (w / 2.0 - r);
        let cy = (py - hy).abs() - (h / 2.0 - r);
        let ox = cx.max(0.0);
        let oy = cy.max(0.0);
        (ox * ox + oy * oy).sqrt() + cx.max(cy).min(0.0) - r
    }

    /// Anti-aliased filled rounded rectangle; interiors are plain spans.
    #[allow(clippy::too_many_arguments)]
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color, a: f32) {
        self.flush();
        if self.px.is_null() {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let (x0, y0) = (x.floor() as i32, y.floor() as i32);
        let (x1, y1) = ((x + w).ceil() as i32, (y + h).ceil() as i32);
        let band = r.ceil() as i32 + 1;
        for yy in y0.max(0)..y1.min(self.h) {
            let edge_row = yy < y0 + band || yy >= y1 - band;
            for xx in x0.max(0)..x1.min(self.w) {
                let edge = edge_row || xx < x0 + band || xx >= x1 - band;
                if edge {
                    let k = (0.5 - Self::coverage(x, y, w, h, r, xx, yy)).clamp(0.0, 1.0);
                    if k > 0.0 {
                        self.blend_px(xx, yy, c, k * a);
                    }
                } else if a >= 0.999 {
                    let end = (x1 - band).min(self.w);
                    self.row(yy, xx, end).fill(0xFF00_0000 | c);
                    // Jump to the right band; the loop variable cannot skip, so handle it here.
                    for rest in end..x1.min(self.w) {
                        let k = (0.5 - Self::coverage(x, y, w, h, r, rest, yy)).clamp(0.0, 1.0);
                        if k > 0.0 {
                            self.blend_px(rest, yy, c, k * a);
                        }
                    }
                    break;
                } else {
                    self.blend_px(xx, yy, c, a);
                }
            }
        }
    }

    /// Anti-aliased rounded outline of the given stroke width.
    #[allow(clippy::too_many_arguments)]
    pub fn round_rect_outline(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, stroke: f32, c: Color, a: f32) {
        self.flush();
        if self.px.is_null() {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let (x0, y0) = (x.floor() as i32 - 1, y.floor() as i32 - 1);
        let (x1, y1) = ((x + w).ceil() as i32 + 1, (y + h).ceil() as i32 + 1);
        let band = (r + stroke).ceil() as i32 + 2;
        for yy in y0.max(0)..y1.min(self.h) {
            let full = yy < y0 + band || yy >= y1 - band;
            let mut xx = x0.max(0);
            let end = x1.min(self.w);
            while xx < end {
                if !full && xx >= x0 + band && xx < x1 - band {
                    xx = x1 - band;
                    continue;
                }
                // Distance from the middle of the stroke, which runs inside the rectangle.
                let d = Self::coverage(x, y, w, h, r, xx, yy) + stroke / 2.0;
                let k = (stroke / 2.0 + 0.5 - d.abs()).clamp(0.0, 1.0);
                if k > 0.0 {
                    self.blend_px(xx, yy, c, k * a);
                }
                xx += 1;
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, c: Color) {
        self.fill(x, y, w, 1, c);
    }

    /// Draws a picture scaled into the rectangle (bilinear), blended over what is there,
    /// clipped to `clip_y0..clip_y1`.
    #[allow(clippy::too_many_arguments)]
    pub fn picture(&mut self, image: &Bgra, x: i32, y: i32, w: i32, h: i32, clip_y0: i32, clip_y1: i32) {
        self.flush();
        if image.width == 0 || image.height == 0 || w <= 0 || h <= 0 {
            return;
        }
        let Some((cx0, cy0, cx1, cy1)) = self.clip(x, y.max(clip_y0), w, (y + h).min(clip_y1) - y.max(clip_y0)) else {
            return;
        };
        let sx = image.width as f32 / w as f32;
        let sy = image.height as f32 / h as f32;
        let (iw, ih) = (image.width as i32, image.height as i32);
        let at = |px: i32, py: i32| -> [f32; 4] {
            let o = ((py.clamp(0, ih - 1) * iw + px.clamp(0, iw - 1)) * 4) as usize;
            let p = &image.pixels[o..o + 4];
            [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
        };
        for yy in cy0..cy1 {
            let fy = ((yy - y) as f32 + 0.5) * sy - 0.5;
            let (y0, ty) = (fy.floor() as i32, fy - fy.floor());
            for xx in cx0..cx1 {
                let fx = ((xx - x) as f32 + 0.5) * sx - 0.5;
                let (x0, tx) = (fx.floor() as i32, fx - fx.floor());
                let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
                let mut v = [0f32; 4];
                for i in 0..4 {
                    let top = a[i] + (b[i] - a[i]) * tx;
                    let bottom = c[i] + (d[i] - c[i]) * tx;
                    v[i] = top + (bottom - top) * ty;
                }
                let color = ((v[2] as u32) << 16) | ((v[1] as u32) << 8) | v[0] as u32;
                self.blend_px(xx, yy, color, v[3] / 255.0);
            }
        }
    }

    /// Composites a square premultiplied BGRA bitmap at (x, y), clipped vertically.
    pub fn blit_premultiplied(&mut self, pixels: &[u32], size: i32, x: i32, y: i32, clip_y0: i32, clip_y1: i32) {
        self.flush();
        if self.px.is_null() {
            return;
        }
        let y0 = y.max(clip_y0).max(0);
        let y1 = (y + size).min(clip_y1).min(self.h);
        let x0 = x.max(0);
        let x1 = (x + size).min(self.w);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        for yy in y0..y1 {
            let src = &pixels[((yy - y) * size + (x0 - x)) as usize..((yy - y) * size + (x1 - x)) as usize];
            let dst = self.row(yy, x0, x1);
            for (d, &s) in dst.iter_mut().zip(src) {
                let a = s >> 24;
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    *d = s | 0xFF00_0000;
                    continue;
                }
                let inv = 255 - a;
                let channel = |shift: u32| -> u32 {
                    let sc = (s >> shift) & 0xFF;
                    let dc = (*d >> shift) & 0xFF;
                    (sc + (dc * inv + 127) / 255).min(255)
                };
                *d = 0xFF00_0000 | (channel(16) << 16) | (channel(8) << 8) | channel(0);
            }
        }
    }

    /// Draw the close mark without depending on a private-use icon font glyph.
    pub fn close_mark(&mut self, (x, y, w, h): (i32, i32, i32, i32), scale: f32, color: Color) {
        self.flush();
        if self.px.is_null() {
            return;
        }
        let cx = x as f32 + w as f32 / 2.0;
        let cy = y as f32 + h as f32 / 2.0;
        let half = 5.5 * scale;
        let stroke = (0.8 * scale).max(0.65);
        let reach = half + stroke + 1.0;
        for yy in (cy - reach).floor() as i32..(cy + reach).ceil() as i32 {
            for xx in (cx - reach).floor() as i32..(cx + reach).ceil() as i32 {
                let dx = (xx as f32 + 0.5 - cx).abs();
                let dy = (yy as f32 + 0.5 - cy).abs();
                let cross = (dx - dy).abs() * std::f32::consts::FRAC_1_SQRT_2;
                let end = (dx + dy - 2.0 * half).max(0.0) * std::f32::consts::FRAC_1_SQRT_2;
                let alpha = (stroke + 0.5 - cross.hypot(end)).clamp(0.0, 1.0);
                self.blend_px(xx, yy, color, alpha);
            }
        }
    }

    pub fn text(&self, font: HFONT, s: &str, rc: (i32, i32, i32, i32), c: Color, flags: u32) {
        if s.is_empty() || rc.2 <= 0 || rc.3 <= 0 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        let mut r = RECT { left: rc.0, top: rc.1, right: rc.0 + rc.2, bottom: rc.1 + rc.3 };
        unsafe {
            SelectObject(self.hdc, font);
            SetTextColor(self.hdc, cr(c));
            DrawTextW(self.hdc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_NOPREFIX);
        }
    }

    pub fn text_width(&self, font: HFONT, s: &str) -> i32 {
        if s.is_empty() {
            return 0;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        let mut sz = SIZE { cx: 0, cy: 0 };
        unsafe {
            SelectObject(self.hdc, font);
            GetTextExtentPoint32W(self.hdc, w.as_ptr(), w.len() as i32, &mut sz);
        }
        sz.cx
    }

    /// Average character width of a font, for cheap line estimates.
    pub fn average_char_width(&self, font: HFONT) -> i32 {
        unsafe {
            SelectObject(self.hdc, font);
            let mut metrics: TEXTMETRICW = std::mem::zeroed();
            GetTextMetricsW(self.hdc, &mut metrics);
            metrics.tmAveCharWidth.max(1)
        }
    }

    pub fn present(&self, dst: HDC, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            GdiFlush();
            BitBlt(dst, x, y, w, h, self.hdc, x, y, SRCCOPY);
        }
    }

    /// Clips GDI text to a rectangle (None removes the clip).
    pub fn clip_rect(&self, r: Option<(i32, i32, i32, i32)>) {
        unsafe {
            match r {
                Some((x, y, w, h)) => {
                    let rgn = CreateRectRgn(x, y, x + w, y + h);
                    SelectClipRgn(self.hdc, rgn);
                    DeleteObject(rgn);
                }
                None => {
                    SelectClipRgn(self.hdc, std::ptr::null_mut());
                }
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp.is_null() {
                SelectObject(self.hdc, self.old);
                DeleteObject(self.bmp);
            }
            DeleteDC(self.hdc);
        }
    }
}

pub const DT_L: u32 = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;
pub const DT_R: u32 = DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;
pub const DT_C: u32 = DT_CENTER | DT_VCENTER | DT_SINGLELINE;
pub const DT_MULTI: u32 = DT_LEFT | DT_TOP | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS | DT_EXPANDTABS;

pub fn make_font(face: &str, px: i32, weight: i32) -> HFONT {
    let f = wide(face);
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            0,
            f.as_ptr(),
        )
    }
}

pub fn font_exists(face: &str) -> bool {
    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, lp: isize) -> i32 {
        *(lp as *mut bool) = true;
        0
    }
    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    lf.lfCharSet = DEFAULT_CHARSET;
    for (i, ch) in face.encode_utf16().take(31).enumerate() {
        lf.lfFaceName[i] = ch;
    }
    let mut exists = false;
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        EnumFontFamiliesExW(dc, &lf, Some(found), &mut exists as *mut bool as isize, 0);
        ReleaseDC(std::ptr::null_mut(), dc);
    }
    exists
}
