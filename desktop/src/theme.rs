use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// The palette is a quiet neutral ground in the manner of T3 Code: near-black
/// or near-white canvas, surfaces separated by hairline borders, neutral fill for
/// the primary action, and signal colors reserved for session state.
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
    /// Counts, hints, and timestamps; never body copy.
    pub faint: u32,
    /// Fill of the primary action.
    pub accent: u32,
    /// Links, focus rings, and the caret.
    pub accent_ink: u32,
    pub accent_hover: u32,
    pub accent_text: u32,
    /// A hairline divider inside a surface.
    pub rule: u32,
    /// Signal tab: an agent is working.
    pub working: u32,
    /// Signal tab: an agent is waiting on the user. Reserved for that state.
    pub waiting: u32,
    /// Signal tab: a finished turn the user has not seen.
    pub done: u32,
    pub danger: u32,
    pub danger_surface: u32,
    pub diff_added: u32,
    pub diff_removed: u32,
    /// Text selection, as `0xRRGGBBAA`.
    pub selection: u32,
    /// Modal backdrop, as `0xRRGGBBAA`.
    pub backdrop: u32,
    /// Translucent window chrome, as `0xRRGGBBAA`.
    pub chrome_glass: u32,
    pub sidebar_glass: u32,
    pub composer_glass: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreset {
    /// Retired presets were all dark, so saved settings fall back to Dark.
    #[default]
    #[serde(alias = "warm", alias = "nord", alias = "dracula")]
    Dark,
    Light,
}

impl ThemePreset {
    pub const ALL: [Self; 2] = [Self::Dark, Self::Light];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    pub const fn colors(self) -> ThemeColors {
        match self {
            Self::Dark => ThemeColors {
                background: 0x1b1b1b,
                sidebar: 0x202020,
                surface: 0x292929,
                surface_high: 0x333333,
                border: 0x383838,
                selected_surface: 0x373737,
                selected_border: 0x858585,
                text: 0xf1f1f1,
                muted: 0xb1b1b1,
                faint: 0xababab,
                accent: 0xdedede,
                accent_ink: 0xdedede,
                accent_hover: 0xf2f2f2,
                accent_text: 0x1b1b1b,
                rule: 0x383838,
                working: 0x8dceb0,
                waiting: 0xe2c28d,
                done: 0xdedede,
                danger: 0xefa7ad,
                danger_surface: 0x2a1616,
                diff_added: 0x263b30,
                diff_removed: 0x462c30,
                selection: 0xffffff35,
                backdrop: 0x000000b3,
                chrome_glass: 0x292929d9,
                sidebar_glass: 0x202020df,
                composer_glass: 0x303030e8,
            },
            Self::Light => ThemeColors {
                background: 0xfafafa,
                sidebar: 0xe4e4e4,
                surface: 0xffffff,
                surface_high: 0xefefef,
                border: 0xd0d0d0,
                selected_surface: 0xededed,
                selected_border: 0x7d7d7d,
                text: 0x202020,
                muted: 0x5a5a5a,
                faint: 0x606060,
                accent: 0x363636,
                accent_ink: 0x444444,
                accent_hover: 0x202020,
                accent_text: 0xffffff,
                rule: 0xd0d0d0,
                working: 0x226347,
                waiting: 0x745319,
                done: 0x444444,
                danger: 0xa13242,
                danger_surface: 0xfdeeee,
                diff_added: 0xe6f5eb,
                diff_removed: 0xfcebeb,
                selection: 0x20202030,
                backdrop: 0x18181b66,
                chrome_glass: 0xf1f1f1dc,
                sidebar_glass: 0xe4e4e4dc,
                composer_glass: 0xffffffd9,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_presets_load_as_dark() {
        for name in ["warm", "nord", "dracula"] {
            let preset: ThemePreset = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(preset, ThemePreset::Dark);
        }
    }
}
