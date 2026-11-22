// SPDX-License-Identifier: GPL-3.0-only

//! Colors from FreeShow's built-in themes (`defaultThemes.ts`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }
}

/// A theme wash such as FreeShow `hover` / `focus` (`rgb(255 255 255 / 0.05)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `frac` is the CSS alpha in thousandths, so 50 is 0.05 (13/255).
    pub const fn from_rgb_frac(r: u8, g: u8, b: u8, frac: u16) -> Self {
        Self {
            r,
            g,
            b,
            a: ((frac as u32 * 255 + 500) / 1000) as u8,
        }
    }
}

const fn white_wash(frac: u16) -> Rgba {
    Rgba::from_rgb_frac(255, 255, 255, frac)
}

const fn black_wash(frac: u16) -> Rgba {
    Rgba::from_rgb_frac(0, 0, 0, frac)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub name: &'static str,
    pub primary: Rgb,
    pub primary_lighter: Rgb,
    pub primary_darker: Rgb,
    pub primary_darkest: Rgb,
    pub text: Rgb,
    pub text_invert: Rgb,
    pub secondary_text: Rgb,
    pub secondary: Rgb,
    pub hover: Rgba,
    pub focus: Rgba,
}

const DEFAULT: ThemePalette = ThemePalette {
    name: "default",
    primary: Rgb::from_hex(0x242832),
    primary_lighter: Rgb::from_hex(0x2f3542),
    primary_darker: Rgb::from_hex(0x191923),
    primary_darkest: Rgb::from_hex(0x12121c),
    text: Rgb::from_hex(0xf0f0ff),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xf0f0ff),
    secondary: Rgb::from_hex(0xf0008c),
    hover: white_wash(50),
    focus: white_wash(100),
};

const DARK: ThemePalette = ThemePalette {
    name: "dark",
    primary: Rgb::from_hex(0x242832),
    primary_lighter: Rgb::from_hex(0x2f3542),
    primary_darker: Rgb::from_hex(0x191923),
    primary_darkest: Rgb::from_hex(0x12121c),
    text: Rgb::from_hex(0xf0f0ff),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xf0f0ff),
    secondary: Rgb::from_hex(0xe64934),
    hover: white_wash(50),
    focus: white_wash(100),
};

const LIGHT: ThemePalette = ThemePalette {
    name: "light",
    primary: Rgb::from_hex(0xdadde2),
    primary_lighter: Rgb::from_hex(0xb0b5c0),
    primary_darker: Rgb::from_hex(0xe8eaed),
    primary_darkest: Rgb::from_hex(0xeff2f6),
    text: Rgb::from_hex(0x333748),
    text_invert: Rgb::from_hex(0xf0f0ff),
    secondary_text: Rgb::from_hex(0x131313),
    secondary: Rgb::from_hex(0xf0008c),
    hover: black_wash(50),
    focus: black_wash(100),
};

const WHITE: ThemePalette = ThemePalette {
    name: "white",
    primary: Rgb::from_hex(0x242832),
    primary_lighter: Rgb::from_hex(0x2f3542),
    primary_darker: Rgb::from_hex(0x191923),
    primary_darkest: Rgb::from_hex(0x12121c),
    text: Rgb::from_hex(0xf0f0ff),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xffffff),
    secondary: Rgb::from_hex(0xfbe1fe),
    hover: white_wash(50),
    focus: white_wash(100),
};

const BLACK: ThemePalette = ThemePalette {
    name: "black",
    primary: Rgb::from_hex(0x000000),
    primary_lighter: Rgb::from_hex(0x424242),
    primary_darker: Rgb::from_hex(0x000000),
    primary_darkest: Rgb::from_hex(0x212121),
    text: Rgb::from_hex(0xffffff),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xffffff),
    secondary: Rgb::from_hex(0xff2e2e),
    hover: white_wash(200),
    focus: white_wash(300),
};

const BLUE: ThemePalette = ThemePalette {
    name: "Blue",
    primary: Rgb::from_hex(0x323743),
    primary_lighter: Rgb::from_hex(0x454c5e),
    primary_darker: Rgb::from_hex(0x23242f),
    primary_darkest: Rgb::from_hex(0x191929),
    text: Rgb::from_hex(0xf0f6ff),
    text_invert: Rgb::from_hex(0x404154),
    secondary_text: Rgb::from_hex(0xf0f6ff),
    secondary: Rgb::from_hex(0x338bff),
    hover: white_wash(50),
    focus: white_wash(100),
};

const SEA: ThemePalette = ThemePalette {
    name: "Sea",
    primary: Rgb::from_hex(0xd5dff1),
    primary_lighter: Rgb::from_hex(0xbfcbe3),
    primary_darker: Rgb::from_hex(0xe0e5ff),
    primary_darkest: Rgb::from_hex(0xecf4fe),
    text: Rgb::from_hex(0x424861),
    text_invert: Rgb::from_hex(0xf0f0ff),
    secondary_text: Rgb::from_hex(0x424861),
    secondary: Rgb::from_hex(0x007db3),
    hover: black_wash(50),
    focus: black_wash(100),
};

const AQUA: ThemePalette = ThemePalette {
    name: "Aqua",
    primary: Rgb::from_hex(0x202020),
    primary_lighter: Rgb::from_hex(0x303030),
    primary_darker: Rgb::from_hex(0x101010),
    primary_darkest: Rgb::from_hex(0x000000),
    text: Rgb::from_hex(0xcccccc),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xeeeeee),
    secondary: Rgb::from_hex(0x00ffbe),
    hover: white_wash(200),
    focus: white_wash(300),
};

const CHARCOAL: ThemePalette = ThemePalette {
    name: "Charcoal",
    primary: Rgb::from_hex(0x292929),
    primary_lighter: Rgb::from_hex(0x303030),
    primary_darker: Rgb::from_hex(0x1e1e1e),
    primary_darkest: Rgb::from_hex(0x121212),
    text: Rgb::from_hex(0xd0d0d0),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0x808080),
    secondary: Rgb::from_hex(0x8f8f8f),
    hover: Rgba::from_rgb_frac(0xd0, 0xd0, 0xd0, 50),
    focus: Rgba::from_rgb_frac(0xd0, 0xd0, 0xd0, 100),
};

const PAPYRUS: ThemePalette = ThemePalette {
    name: "Papyrus",
    primary: Rgb::from_hex(0xeac6a4),
    primary_lighter: Rgb::from_hex(0xd6b59a),
    primary_darker: Rgb::from_hex(0xffd1b3),
    primary_darkest: Rgb::from_hex(0xd7b99d),
    text: Rgb::from_hex(0x000242),
    text_invert: Rgb::from_hex(0x000242),
    secondary_text: Rgb::from_hex(0x000242),
    secondary: Rgb::from_hex(0x000361),
    hover: Rgba::from_rgb_frac(0xea, 0xc6, 0xa4, 200),
    focus: Rgba::from_rgb_frac(0xea, 0xc6, 0xa4, 300),
};

const TERMINAL: ThemePalette = ThemePalette {
    name: "Terminal",
    primary: Rgb::from_hex(0x202020),
    primary_lighter: Rgb::from_hex(0x303030),
    primary_darker: Rgb::from_hex(0x101010),
    primary_darkest: Rgb::from_hex(0x000000),
    text: Rgb::from_hex(0xcccccc),
    text_invert: Rgb::from_hex(0x131313),
    secondary_text: Rgb::from_hex(0xf1f1f1),
    secondary: Rgb::from_hex(0x00ff00),
    hover: white_wash(200),
    focus: white_wash(300),
};

/// Every built-in theme in the order FreeShow lists them.
pub const BUILT_IN_THEMES: [ThemePalette; 11] = [
    DEFAULT, DARK, LIGHT, WHITE, BLACK, BLUE, SEA, AQUA, CHARCOAL, PAPYRUS, TERMINAL,
];

/// Returns a built-in palette by its FreeShow key.
///
/// Keys are matched without regard to case, so a settings file that stored the
/// display name (`Blue`) still finds `blue`. Unknown names fall back to
/// FreeShow's default theme.
pub fn theme_by_name(name: &str) -> ThemePalette {
    BUILT_IN_THEMES
        .into_iter()
        .find(|theme| theme.name.eq_ignore_ascii_case(name))
        .unwrap_or(DEFAULT)
}
