//! Light and dark palettes close to the Windows 11 flyouts.

use super::gfx::{mix, Color};
use crate::settings::Theme;
use crate::system;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub background: Color,
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

pub fn is_dark(theme: Theme) -> bool {
    match theme {
        Theme::System => !system::apps_use_light_theme(),
        Theme::Light => false,
        Theme::Dark => true,
    }
}

pub fn palette(theme: Theme) -> Palette {
    let dark = is_dark(theme);
    let accent = system::accent_color();
    if dark {
        // Windows uses a lighter accent shade on dark surfaces.
        let accent = mix(accent, 0xFFFFFF, 0.45);
        Palette {
            dark,
            background: 0x202020,
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
            background: 0xF3F3F3,
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
