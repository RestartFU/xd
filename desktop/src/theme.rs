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
    pub selected_surface: u32,
    pub selected_border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_ink: u32,
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
                background: 0x11110f,
                sidebar: 0x181818,
                surface: 0x202020,
                surface_high: 0x282826,
                border: 0x303030,
                selected_surface: 0x2b201c,
                selected_border: 0x704333,
                text: 0xf1f1f1,
                muted: 0xa0a0a0,
                accent: 0xf07a55,
                accent_ink: 0xf07a55,
                accent_hover: 0xf58966,
                accent_text: 0x1b0e09,
            },
            Self::Light => ThemeColors {
                background: 0xf7f7f5,
                sidebar: 0xffffff,
                surface: 0xfafafa,
                surface_high: 0xf3f3f1,
                border: 0xebebeb,
                selected_surface: 0xfdf0eb,
                selected_border: 0xf1d2cb,
                text: 0x202020,
                muted: 0x6f6f6f,
                accent: 0xe96a43,
                accent_ink: 0xa34226,
                accent_hover: 0xda5d37,
                accent_text: 0x2a1008,
            },
            Self::Warm => ThemeColors {
                background: 0x17110d,
                sidebar: 0x100b08,
                surface: 0x201711,
                surface_high: 0x2c2018,
                border: 0x49362a,
                selected_surface: 0x352219,
                selected_border: 0x764733,
                text: 0xf7eee7,
                muted: 0xbba89a,
                accent: 0xee7650,
                accent_ink: 0xee7650,
                accent_hover: 0xf38a66,
                accent_text: 0x1c0f08,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_theme_matches_the_flat_coral_session_board() {
        let colors = ThemePreset::Light.colors();

        assert_eq!(colors.background, 0xf7f7f5);
        assert_eq!(colors.sidebar, 0xffffff);
        assert_eq!(colors.surface, 0xfafafa);
        assert_eq!(colors.border, 0xebebeb);
        assert_eq!(colors.accent, 0xe96a43);
    }
}
