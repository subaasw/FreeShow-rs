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

/// Returns a built-in palette. Unknown names fall back to FreeShow's default theme.
pub fn theme_by_name(name: &str) -> ThemePalette {
    match name {
        "default" => DEFAULT,
        "dark" => DARK,
        "light" => LIGHT,
        _ => DEFAULT,
    }
}
