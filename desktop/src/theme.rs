use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// Dark and Light are a quiet neutral ground in the manner of T3 Code: near-black
/// or near-white canvas, surfaces separated by hairline borders, neutral fill for
/// the primary action, and signal colors reserved for session state. Modern keeps
/// that structure on a cool slate ground and gives the primary action, links,
/// focus and selection an indigo accent.
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
    /// Assistant brand marks, independent of selection and activity state.
    pub codex_icon: u32,
    pub claude_icon: u32,
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
    /// Title bars, the chat tab row, and the settings panel.
    pub chrome: u32,
    /// The message composer.
    pub composer: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreset {
    /// Retired presets were all dark, so saved settings fall back to Dark.
    #[default]
    #[serde(alias = "warm", alias = "nord", alias = "dracula")]
    Dark,
    Light,
    Modern,
}

impl ThemePreset {
    pub const ALL: [Self; 3] = [Self::Dark, Self::Light, Self::Modern];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Modern => "Modern",
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
                codex_icon: 0xffffff,
                claude_icon: 0xd97757,
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
                chrome: 0x292929,
                composer: 0x303030,
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
                codex_icon: 0x202020,
                claude_icon: 0xb85c3f,
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
                chrome: 0xf1f1f1,
                composer: 0xffffff,
            },
            Self::Modern => ThemeColors {
                background: 0x0e1016,
                sidebar: 0x12141c,
                surface: 0x191c26,
                surface_high: 0x222633,
                border: 0x2a2e3c,
                selected_surface: 0x252a3a,
                selected_border: 0x7178e6,
                text: 0xedeff5,
                codex_icon: 0xffffff,
                claude_icon: 0xd97757,
                muted: 0xa6acbd,
                faint: 0x8d93a6,
                accent: 0x8b8ff8,
                accent_ink: 0xa3a7ff,
                accent_hover: 0xa6a9ff,
                accent_text: 0x0e1016,
                rule: 0x262a37,
                working: 0x5fd3a3,
                waiting: 0xf0c06a,
                done: 0xa3a7ff,
                danger: 0xff8f9d,
                danger_surface: 0x2c1520,
                diff_added: 0x173328,
                diff_removed: 0x3b1d28,
                selection: 0x8b8ff840,
                backdrop: 0x05060ab3,
                chrome: 0x171a23,
                composer: 0x1d2130,
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
