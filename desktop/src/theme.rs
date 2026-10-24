use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// The palette follows a library card catalog: a neutral cabinet ground,
/// card-stock surfaces with a red ruled line, bone or ink for the primary
/// action, and signal-tab colors that are reserved for session state.
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
    /// The red ruled line under a card's heading.
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
                background: 0x121314,
                sidebar: 0x17181a,
                surface: 0x1e1f22,
                surface_high: 0x27292c,
                border: 0x313338,
                selected_surface: 0x2b2d31,
                selected_border: 0xe8e5de,
                text: 0xeceae5,
                muted: 0xaaa9a3,
                faint: 0x8b8a85,
                accent: 0xe8e5de,
                accent_ink: 0x93b5e9,
                accent_hover: 0xffffff,
                accent_text: 0x121314,
                rule: 0xa8493c,
                working: 0x72b783,
                waiting: 0xeaa93a,
                done: 0x86a9e4,
                danger: 0xec7064,
                danger_surface: 0x3b201e,
                diff_added: 0x1b2b20,
                diff_removed: 0x33201e,
                selection: 0x93b5e955,
                backdrop: 0x000000a6,
            },
            Self::Light => ThemeColors {
                background: 0xf7f7f5,
                sidebar: 0xeeeeeb,
                surface: 0xffffff,
                surface_high: 0xe9e9e5,
                border: 0xd7d6d0,
                selected_surface: 0xe6e5e0,
                selected_border: 0x1b1c1e,
                text: 0x1b1c1e,
                muted: 0x56554f,
                faint: 0x64635d,
                accent: 0x1b1c1e,
                accent_ink: 0x2b5897,
                accent_hover: 0x3a3c40,
                accent_text: 0xffffff,
                rule: 0xc2483b,
                working: 0x276b3c,
                waiting: 0x8c5100,
                done: 0x2b5897,
                danger: 0xb2342a,
                danger_surface: 0xf8e2df,
                diff_added: 0xe2f0e5,
                diff_removed: 0xf8e4e1,
                selection: 0x2b589740,
                backdrop: 0x1b1c1e66,
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
