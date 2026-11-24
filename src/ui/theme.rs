//! Windows palettes and the six Better series themes.

use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

use super::gfx::{mix, Color};
use crate::settings::Theme;
use crate::system;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub contrast: bool,
    pub motifs: bool,
    pub background: Color,
    pub header: Color,
    pub border: Color,
    pub card: Color,
    pub card_hover: Color,
    pub card_border: Color,
    pub text: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub field: Color,
    pub field_border: Color,
    pub control_hover: Color,
    pub accent: Color,
    /// Text on accent-coloured surfaces.
    pub on_accent: Color,
    pub focus: Color,
    pub banner: Color,
}

pub fn palette(theme: Theme) -> Palette {
    if high_contrast() {
        contrast_palette()
    } else {
        normal_palette(theme)
    }
}

pub fn preview_palettes() -> [Palette; Theme::ALL.len()] {
    if high_contrast() {
        [contrast_palette(); Theme::ALL.len()]
    } else {
        Theme::ALL.map(normal_palette)
    }
}

fn normal_palette(theme: Theme) -> Palette {
    if let Some(palette) = custom_palette(theme) {
        return palette;
    }
    let dark = theme == Theme::Dark || (theme == Theme::System && !system::apps_use_light_theme());
    standard(dark, system::accent_color())
}

fn standard(dark: bool, accent: Color) -> Palette {
    if dark {
        // Windows uses a lighter accent shade on dark surfaces.
        let accent = mix(accent, 0xFFFFFF, 0.45);
        Palette {
            dark,
            contrast: false,
            motifs: false,
            background: 0x202020,
            header: 0x202020,
            border: 0x404040,
            card: 0x2D2D2D,
            card_hover: 0x353535,
            card_border: 0x383838,
            text: 0xFFFFFF,
            secondary: 0xCFCFCF,
            tertiary: 0x9D9D9D,
            field: 0x2A2A2A,
            field_border: 0x454545,
            control_hover: 0x383838,
            accent,
            on_accent: 0x000000,
            focus: 0xFFFFFF,
            banner: 0x3B3220,
        }
    } else {
        let accent = mix(accent, 0x000000, 0.12);
        Palette {
            dark,
            contrast: false,
            motifs: false,
            background: 0xF3F3F3,
            header: 0xF3F3F3,
            border: 0xD5D5D5,
            card: 0xFFFFFF,
            card_hover: 0xF9F9F9,
            card_border: 0xE5E5E5,
            text: 0x1B1B1B,
            secondary: 0x5D5D5D,
            tertiary: 0x8A8A8A,
            field: 0xFFFFFF,
            field_border: 0xD0D0D0,
            control_hover: 0xE8E8E8,
            accent,
            on_accent: 0xFFFFFF,
            focus: 0x000000,
            banner: 0xFFF4CE,
        }
    }
}

fn custom_palette(theme: Theme) -> Option<Palette> {
    // Content, title, chrome, field, text, muted, edge, active and accent match BetterExplorer.
    let (dark, [background, header, card, field, text, secondary, border, active, accent]) = match theme {
        Theme::DeepSpace => {
            (true, [0x0F1223, 0x1B193F, 0x141831, 0x232840, 0xEFEFFF, 0xB4BDE0, 0x414670, 0x2F2B5B, 0xC2ABFF])
        }
        Theme::PaperWhite => {
            (false, [0xFFFDF6, 0xF2ECDC, 0xF7F3E8, 0xFFFBF0, 0x232B39, 0x555E6F, 0xCAC0A8, 0xE0E9F6, 0x2B5091])
        }
        Theme::CandyRush => {
            (false, [0xFFF8FC, 0xF9D5EB, 0xFBE0F0, 0xFFF3FA, 0x431631, 0x6B3757, 0xD99BC1, 0xF5C1E0, 0x971B5F])
        }
        Theme::Frost => {
            (false, [0xF8FDFF, 0xD5EDF8, 0xDFF2F9, 0xF3FCFF, 0x193949, 0x426576, 0xA4C8D7, 0xC5E5F2, 0x175C81])
        }
        Theme::Eclipse => {
            (true, [0x161312, 0x2B1E19, 0x231C19, 0x35261F, 0xFCEFE1, 0xCFB49E, 0x5B4233, 0x432F26, 0xFFAF71])
        }
        Theme::CyberGrid => {
            (true, [0x09191B, 0x0D2C2B, 0x0D2424, 0x133636, 0xE1FCF2, 0x93C4B7, 0x31685E, 0x14433B, 0x67F4C1])
        }
        Theme::System | Theme::Light | Theme::Dark => return None,
    };
    Some(Palette {
        dark,
        contrast: false,
        motifs: true,
        background,
        header,
        border,
        card,
        card_hover: active,
        card_border: mix(card, border, 0.7),
        text,
        secondary,
        tertiary: secondary,
        field,
        field_border: mix(border, secondary, 0.55),
        control_hover: active,
        accent,
        on_accent: if dark { background } else { 0xFFFFFF },
        focus: accent,
        banner: if dark { 0x3B3220 } else { 0xFFF4CE },
    })
}

fn high_contrast() -> bool {
    let mut contrast: HIGHCONTRASTW = unsafe { std::mem::zeroed() };
    contrast.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
    unsafe {
        SystemParametersInfoW(SPI_GETHIGHCONTRAST, contrast.cbSize, (&mut contrast as *mut HIGHCONTRASTW).cast(), 0)
            != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

fn contrast_palette() -> Palette {
    let get = |index| super::gfx::cr(unsafe { GetSysColor(index) });
    let background = get(COLOR_WINDOW);
    let text = get(COLOR_WINDOWTEXT);
    let accent = get(COLOR_HIGHLIGHT);
    Palette {
        dark: false,
        contrast: true,
        motifs: false,
        background,
        header: background,
        border: text,
        card: background,
        card_hover: background,
        card_border: text,
        text,
        secondary: text,
        tertiary: text,
        field: background,
        field_border: text,
        control_hover: background,
        accent,
        on_accent: get(COLOR_HIGHLIGHTTEXT),
        focus: text,
        banner: background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color, b: Color) -> f64 {
        fn luminance(color: Color) -> f64 {
            let channel = |shift: u32| {
                let s = f64::from((color >> shift) & 255) / 255.0;
                if s <= 0.04045 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        }
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn custom_themes_keep_text_and_focus_legible() {
        for theme in Theme::ALL.into_iter().filter(|theme| theme.is_custom()) {
            let p = custom_palette(theme).unwrap();
            for (foreground, background) in [
                (p.text, p.background),
                (p.text, p.header),
                (p.text, p.card),
                (p.text, p.card_hover),
                (p.text, p.banner),
                (p.secondary, p.card),
                (p.tertiary, p.field),
                (p.on_accent, p.accent),
            ] {
                assert!(contrast(foreground, background) >= 4.5, "{theme:?}: {foreground:06X} on {background:06X}");
            }
            assert!(contrast(p.focus, p.card) >= 3.0, "{theme:?} focus");
        }
    }
}
