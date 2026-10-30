use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// The palette is a quiet neutral ground in the manner of T3 Code: near-black
/// or near-white canvas, surfaces separated by hairline borders, one blue for
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
                background: 0x0b0b0c,
                sidebar: 0x0f0f10,
                surface: 0x161618,
                surface_high: 0x1d1d20,
                border: 0x252528,
                selected_surface: 0x1f1f23,
                selected_border: 0x3f3f46,
                text: 0xf1f3f7,
                muted: 0xa3a3a8,
                faint: 0x76767d,
                accent: 0x2f6bf2,
                accent_ink: 0x7aa5ff,
                accent_hover: 0x2558d6,
                accent_text: 0xffffff,
                rule: 0x252528,
                working: 0x34d399,
                waiting: 0xfbbf24,
                done: 0x6aa1ff,
                danger: 0xf87171,
                danger_surface: 0x2a1616,
                diff_added: 0x10261a,
                diff_removed: 0x2c1515,
                selection: 0x2f6bf255,
                backdrop: 0x000000b3,
            },
            Self::Light => ThemeColors {
                background: 0xfcfcfc,
                sidebar: 0xf5f5f6,
                surface: 0xffffff,
                surface_high: 0xefeff1,
                border: 0xe4e4e7,
                selected_surface: 0xe9e9ec,
                selected_border: 0xa1a1aa,
                text: 0x18181b,
                muted: 0x52525b,
                faint: 0x6b6b74,
                accent: 0x1d4ed8,
                accent_ink: 0x1d4ed8,
                accent_hover: 0x1e40af,
                accent_text: 0xffffff,
                rule: 0xe4e4e7,
                working: 0x047857,
                waiting: 0x92400e,
                done: 0x1d4ed8,
                danger: 0xb91c1c,
                danger_surface: 0xfdeeee,
                diff_added: 0xe6f5eb,
                diff_removed: 0xfcebeb,
                selection: 0x1d4ed840,
                backdrop: 0x18181b66,
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
