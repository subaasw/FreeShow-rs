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
