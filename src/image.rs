//! Bitmaps: decoding clipboard DIBs, thumbnails and PNG encoding.
//!
//! Pixels are kept as straight (not premultiplied) BGRA, the byte order of 32-bit DIBs.

use std::io::Cursor;

/// Largest bitmap accepted from the clipboard (pixels), to bound memory use.
pub const MAX_PIXELS: u64 = 16384 * 16384;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    /// width * height * 4 bytes, top-down rows.
    pub pixels: Vec<u8>,
}

impl Bgra {
    pub fn new(width: u32, height: u32) -> Bgra {
        Bgra { width, height, pixels: vec![0; width as usize * height as usize * 4] }
    }

    pub fn is_opaque(&self) -> bool {
        self.pixels.chunks_exact(4).all(|p| p[3] == 255)
    }

    /// Box-filtered downscale so the picture fits into `max_w` x `max_h`. Never upscales.
    pub fn fit(&self, max_w: u32, max_h: u32) -> Bgra {
        let (w, h) = fit_size(self.width, self.height, max_w, max_h);
        if w == self.width && h == self.height {
            return self.clone();
        }
        self.resample(w, h)
    }

    /// Area-averaging resample (alpha-weighted, so transparent edges do not darken).
    pub fn resample(&self, w: u32, h: u32) -> Bgra {
        let mut out = Bgra::new(w.max(1), h.max(1));
        if self.width == 0 || self.height == 0 {
            return out;
        }
        let sx = self.width as f32 / out.width as f32;
        let sy = self.height as f32 / out.height as f32;
        let stride = self.width as usize * 4;
        for oy in 0..out.height {
            let y0 = (oy as f32 * sy) as u32;
            let y1 = (((oy + 1) as f32 * sy).ceil() as u32).clamp(y0 + 1, self.height);
            for ox in 0..out.width {
                let x0 = (ox as f32 * sx) as u32;
                let x1 = (((ox + 1) as f32 * sx).ceil() as u32).clamp(x0 + 1, self.width);
                let (mut b, mut g, mut r, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
                for y in y0..y1 {
                    let row = &self.pixels[y as usize * stride..];
                    for x in x0..x1 {
                        let p = &row[x as usize * 4..x as usize * 4 + 4];
                        let alpha = p[3] as u32;
                        b += p[0] as u32 * alpha;
                        g += p[1] as u32 * alpha;
                        r += p[2] as u32 * alpha;
                        a += alpha;
                        n += 1;
                    }
                }
                let o = (oy as usize * out.width as usize + ox as usize) * 4;
                if let (Some(b), Some(g), Some(r)) = (b.checked_div(a), g.checked_div(a), r.checked_div(a)) {
                    out.pixels[o] = b as u8;
                    out.pixels[o + 1] = g as u8;
                    out.pixels[o + 2] = r as u8;
                }
                out.pixels[o + 3] = (a / n.max(1)) as u8;
            }
        }
        out
    }

    /// A 64-bit fingerprint of size and pixels, used to group duplicate copies.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.width.hash(&mut hasher);
        self.height.hash(&mut hasher);
        self.pixels.hash(&mut hasher);
        hasher.finish()
    }

    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(self.pixels.len() / 4);
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            let opaque = self.is_opaque();
            encoder.set_color(if opaque { png::ColorType::Rgb } else { png::ColorType::Rgba });
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            let data: Vec<u8> = if opaque {
                self.pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect()
            } else {
                self.pixels.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()
            };
            writer.write_image_data(&data).map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    pub fn from_png(bytes: &[u8]) -> Result<Bgra, String> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let (w, h) = {
            let info = reader.info();
            (info.width, info.height)
        };
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err("image too large".into());
        }
        let mut buffer = vec![0; reader.output_buffer_size().ok_or("image too large")?];
        let frame = reader.next_frame(&mut buffer).map_err(|e| e.to_string())?;
        let data = &buffer[..frame.buffer_size()];
        let mut out = Bgra::new(frame.width, frame.height);
        let channels = match frame.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return Err("unexpanded palette".into()),
        };
        for (o, p) in out.pixels.chunks_exact_mut(4).zip(data.chunks_exact(channels)) {
            let (r, g, b, a) = match channels {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            o.copy_from_slice(&[b, g, r, a]);
        }
        Ok(out)
    }

    /// A packed CF_DIB: BITMAPINFOHEADER, 32 bpp, BI_RGB, bottom-up, alpha in the fourth byte.
    pub fn to_dib(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(40 + self.pixels.len());
        header(&mut out, 40, self.width as i32, self.height as i32, 32, 0, self.pixels.len() as u32);
        let stride = self.width as usize * 4;
        for row in self.pixels.chunks_exact(stride.max(1)).rev() {
            out.extend_from_slice(row);
        }
        out
    }

    /// A packed CF_DIBV5 with an explicit alpha mask, preferred by apps that keep transparency.
    pub fn to_dibv5(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(124 + self.pixels.len());
        header(&mut out, 124, self.width as i32, self.height as i32, 32, 3, self.pixels.len() as u32);
        for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
            out.extend_from_slice(&mask.to_le_bytes());
        }
        out.extend_from_slice(&0x7352_4742u32.to_le_bytes()); // LCS_sRGB
        out.resize(124, 0);
        // bV5Intent = LCS_GM_IMAGES
        out[108..112].copy_from_slice(&4u32.to_le_bytes());
        let stride = self.width as usize * 4;
        for row in self.pixels.chunks_exact(stride.max(1)).rev() {
            out.extend_from_slice(row);
        }
        out
    }

    pub fn to_bmp_file(&self) -> Vec<u8> {
        let dib = self.to_dib();
        let mut out = Vec::with_capacity(14 + dib.len());
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(14 + dib.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(14u32 + 40).to_le_bytes());
        out.extend_from_slice(&dib);
        out
    }
}

fn header(out: &mut Vec<u8>, size: u32, w: i32, h: i32, bits: u16, compression: u32, image_size: u32) {
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(&compression.to_le_bytes());
    out.extend_from_slice(&image_size.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
}

pub fn fit_size(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1))
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Decodes a packed DIB (CF_DIB or CF_DIBV5 contents).
///
/// Supports 1/4/8 bpp palettes, 16/24/32 bpp with BI_RGB or BI_BITFIELDS, top-down and
/// bottom-up rows, and BI_PNG. RLE and JPEG payloads return an error.
pub fn decode_dib(data: &[u8]) -> Result<Bgra, String> {
    let bad = || "malformed bitmap".to_string();
    let header_size = u32_at(data, 0).ok_or_else(bad)? as usize;
    if header_size < 40 || header_size > data.len() {
        return Err(bad());
    }
    let width = u32_at(data, 4).ok_or_else(bad)? as i32;
    let raw_height = u32_at(data, 8).ok_or_else(bad)? as i32;
    let bits = u16_at(data, 14).ok_or_else(bad)?;
    let compression = u32_at(data, 16).ok_or_else(bad)?;
    let used_colors = u32_at(data, 32).ok_or_else(bad)? as usize;
    if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
        return Err(bad());
    }
    let top_down = raw_height < 0;
    let height = raw_height.unsigned_abs();
    let width = width as u32;
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err("image too large".into());
    }

    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    const BI_PNG: u32 = 5;
    const BI_ALPHABITFIELDS: u32 = 6;

    if compression == BI_PNG {
        return Bgra::from_png(data.get(header_size..).ok_or_else(bad)?);
    }

    // Colour masks follow a 40-byte header for BI_BITFIELDS; V4/V5 headers contain them.
    let mut masks_end = header_size;
    let masks = match compression {
        BI_BITFIELDS | BI_ALPHABITFIELDS => {
            // Masks sit at offset 40 in both cases: after a plain header, or inside V4/V5.
            let base = 40;
            let count = if compression == BI_ALPHABITFIELDS || header_size >= 56 { 4 } else { 3 };
            let m: Vec<u32> = (0..count).map(|i| u32_at(data, base + i * 4).unwrap_or(0)).collect();
            if header_size == 40 {
                masks_end = 40 + count * 4;
            }
            Some([m[0], m[1], m[2], *m.get(3).unwrap_or(&0)])
        }
        BI_RGB => match bits {
            16 => Some([0x7C00, 0x03E0, 0x001F, 0]),
            32 => Some([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]),
            _ => None,
        },
        _ => return Err("unsupported bitmap compression".into()),
    };

    let palette_len = if bits <= 8 {
        if used_colors == 0 {
            1usize << bits
        } else {
            used_colors.min(256)
        }
    } else {
        0
    };
    let palette_start = masks_end;
    let palette: Vec<[u8; 4]> = (0..palette_len)
        .map(|i| {
            let o = palette_start + i * 4;
            data.get(o..o + 4).map(|p| [p[0], p[1], p[2], 255]).unwrap_or([0, 0, 0, 255])
        })
        .collect();
    let pixels_start = palette_start + palette_len * 4;
    let stride = (width as usize * bits as usize).div_ceil(32) * 4;
    let needed = stride * height as usize;
    let pixel_data = data.get(pixels_start..pixels_start + needed).ok_or_else(bad)?;

    let mut out = Bgra::new(width, height);
    let mut any_alpha = false;
    for row in 0..height as usize {
        let source_row = if top_down { row } else { height as usize - 1 - row };
        let src = &pixel_data[source_row * stride..source_row * stride + stride];
        let dst = &mut out.pixels[row * width as usize * 4..(row + 1) * width as usize * 4];
        for x in 0..width as usize {
            let px: [u8; 4] = match bits {
                1 | 4 | 8 => {
                    let bit_pos = x * bits as usize;
                    let byte = src[bit_pos / 8];
                    let shift = 8 - bits as usize - (bit_pos % 8);
                    let index = ((byte >> shift) & ((1u16 << bits) - 1) as u8) as usize;
                    *palette.get(index).unwrap_or(&[0, 0, 0, 255])
                }
                24 => [src[x * 3], src[x * 3 + 1], src[x * 3 + 2], 255],
                16 | 32 => {
                    let value = if bits == 16 {
                        u16_at(src, x * 2).unwrap_or(0) as u32
                    } else {
                        u32_at(src, x * 4).unwrap_or(0)
                    };
                    let m = masks.ok_or_else(bad)?;
                    let a = if m[3] != 0 { channel(value, m[3]) } else { 255 };
                    [channel(value, m[2]), channel(value, m[1]), channel(value, m[0]), a]
                }
                _ => return Err("unsupported bit depth".into()),
            };
            if px[3] != 0 {
                any_alpha = true;
            }
            dst[x * 4..x * 4 + 4].copy_from_slice(&px);
        }
    }
    // Many programs leave the fourth byte of 32-bit DIBs at zero: that means opaque.
    if !any_alpha {
        for p in out.pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Ok(out)
}

fn channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let v = (value & mask) >> shift;
    if max == 255 {
        v as u8
    } else {
        ((v * 255 + max / 2) / max) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Bgra {
        let mut image = Bgra::new(3, 2);
        for (i, p) in image.pixels.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[i as u8 * 10, 100, 200, 255]);
        }
        image
    }

    #[test]
    fn dib_round_trip() {
        let image = sample();
        assert_eq!(decode_dib(&image.to_dib()).unwrap(), image);
        assert_eq!(decode_dib(&image.to_dibv5()).unwrap(), image);
    }

    #[test]
    fn png_round_trip_keeps_alpha() {
        let mut image = sample();
        image.pixels[3] = 0;
        image.pixels[0..3].copy_from_slice(&[0, 0, 0]);
        let png = image.to_png().unwrap();
        assert_eq!(Bgra::from_png(&png).unwrap(), image);
        let opaque = sample();
        assert_eq!(Bgra::from_png(&opaque.to_png().unwrap()).unwrap(), opaque);
    }

    #[test]
    fn decodes_24_bit_bottom_up_with_padding() {
        // 1x2 image, 24 bpp: each row is 3 bytes + 1 padding byte.
        let mut dib = Vec::new();
        header(&mut dib, 40, 1, 2, 24, 0, 8);
        dib.extend_from_slice(&[1, 2, 3, 0]); // bottom row
        dib.extend_from_slice(&[4, 5, 6, 0]); // top row
        let image = decode_dib(&dib).unwrap();
        assert_eq!(image.pixels, vec![4, 5, 6, 255, 1, 2, 3, 255]);
    }

    #[test]
    fn decodes_palette_and_565() {
        let mut dib = Vec::new();
        header(&mut dib, 40, 2, -1, 1, 0, 4);
        dib.extend_from_slice(&[0, 0, 0, 0, 255, 255, 255, 0]);
        dib.extend_from_slice(&[0b0100_0000, 0, 0, 0]);
        let image = decode_dib(&dib).unwrap();
        assert_eq!(image.pixels, vec![0, 0, 0, 255, 255, 255, 255, 255]);

        let mut dib = Vec::new();
        header(&mut dib, 40, 1, 1, 16, 3, 4);
        for m in [0xF800u32, 0x07E0, 0x001F] {
            dib.extend_from_slice(&m.to_le_bytes());
        }
        dib.extend_from_slice(&[0x00, 0xF8, 0, 0]); // pure red in 565
        assert_eq!(decode_dib(&dib).unwrap().pixels, vec![0, 0, 255, 255]);
    }

    #[test]
    fn zero_alpha_32_bit_means_opaque() {
        let mut dib = Vec::new();
        header(&mut dib, 40, 1, 1, 32, 0, 4);
        dib.extend_from_slice(&[9, 8, 7, 0]);
        assert_eq!(decode_dib(&dib).unwrap().pixels, vec![9, 8, 7, 255]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_dib(&[]).is_err());
        assert!(decode_dib(&[40, 0, 0, 0, 1, 2]).is_err());
        let mut dib = Vec::new();
        header(&mut dib, 40, 100, 100, 32, 0, 0);
        assert!(decode_dib(&dib).is_err(), "missing pixel data");
        let mut huge = Vec::new();
        header(&mut huge, 40, 1 << 20, 1 << 20, 32, 0, 0);
        assert!(decode_dib(&huge).is_err());
    }

    #[test]
    fn thumbnails_keep_aspect_ratio() {
        assert_eq!(fit_size(1920, 1080, 320, 160), (284, 160));
        assert_eq!(fit_size(100, 50, 320, 160), (100, 50));
        let big = Bgra { width: 8, height: 4, pixels: vec![200; 8 * 4 * 4] };
        let small = big.fit(4, 4);
        assert_eq!((small.width, small.height), (4, 2));
        assert!(small.pixels.iter().all(|&v| v == 200));
    }
}
