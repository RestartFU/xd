use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    pub background: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_high: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_text: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreset {
    #[default]
    Dark,
    Light,
    Warm,
}

impl ThemePreset {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::Warm];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Warm => "Warm",
        }
    }

    pub const fn colors(self) -> ThemeColors {
        match self {
            Self::Dark => ThemeColors {
                background: 0x0a0a0c,
                sidebar: 0x060607,
                surface: 0x101013,
                surface_high: 0x1a1a1e,
                border: 0x2a2a2d,
                text: 0xf2f2f4,
                muted: 0xa8a8ad,
                accent: 0x6b8cff,
                accent_hover: 0x7b98ff,
                accent_text: 0x090b10,
            },
            Self::Light => ThemeColors {
                background: 0xf7f6f2,
                sidebar: 0xeeece6,
                surface: 0xffffff,
                surface_high: 0xe8e5de,
                border: 0xc9c5bc,
                text: 0x1c1b19,
                muted: 0x625f59,
                accent: 0x3159c6,
                accent_hover: 0x294cae,
                accent_text: 0xffffff,
            },
            Self::Warm => ThemeColors {
                background: 0x17110d,
                sidebar: 0x100b08,
                surface: 0x201711,
                surface_high: 0x2c2018,
                border: 0x49362a,
                text: 0xf7eee7,
                muted: 0xbba89a,
                accent: 0xe98949,
                accent_hover: 0xf2a260,
                accent_text: 0x1c0f08,
            },
        }
    }
}
