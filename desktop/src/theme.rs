use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// Dark and Light are a quiet neutral ground in the manner of T3 Code: near-black
/// or near-white canvas, surfaces separated by hairline borders, neutral fill for
/// the primary action, and signal colors reserved for session state. Modern and
/// Modern Light are frosted glass in the manner of macOS: a soft colored wash
/// behind the app, translucent panes over it, and an indigo accent. See [`Glass`].
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
    /// Frosted glass, for the themes that have it. The opaque roles above stay
    /// complete, so anything that does not draw glass still looks right.
    pub glass: Option<Glass>,
}

/// Glass drawn inside the app; the window itself stays opaque, so nothing
/// outside it ever shows through.
///
/// The window paints a diagonal wash with a glow in one corner. Page canvases (the chat, a project's page,
/// the session column) leave it showing, and the sidebar, title bars, tab rows
/// and composer are translucent panes over it with a hairline edge. Reading
/// surfaces such as the terminal, code blocks, cards, menus and dialogs keep
/// their opaque roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glass {
    /// The wash runs from this color at the top left...
    pub wash_from: u32,
    /// ...to this one at the bottom right.
    pub wash_to: u32,
    /// A second wash from the top right corner, fading out before the middle,
    /// as `0xRRGGBBAA`.
    pub glow: u32,
    /// The sidebar, title bars and tab rows, as `0xRRGGBBAA`.
    pub pane: u32,
    /// The message composer, as `0xRRGGBBAA`.
    pub composer: u32,
    /// The hairline along a pane's edge, as `0xRRGGBBAA`: light on the dark
    /// wash, dark on the light one.
    pub edge: u32,
}

impl Glass {
    /// The opaque color a translucent `0xRRGGBBAA` fill shows over `under`.
    pub const fn over(fill: u32, under: u32) -> u32 {
        let alpha = fill & 0xff;
        let top = fill >> 8;
        let mut out = 0;
        let mut shift = 0;
        while shift <= 16 {
            let a = (top >> shift) & 0xff;
            let b = (under >> shift) & 0xff;
            out |= ((a * alpha + b * (255 - alpha) + 127) / 255) << shift;
            shift += 8;
        }
        out
    }
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
    ModernLight,
}

impl ThemePreset {
    pub const ALL: [Self; 4] = [Self::Dark, Self::Light, Self::Modern, Self::ModernLight];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::Modern => "Modern",
            Self::ModernLight => "Modern Light",
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
                glass: None,
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
                glass: None,
            },
            Self::Modern => ThemeColors {
                background: 0x11141d,
                sidebar: 0x1e2236,
                surface: 0x1b1f2f,
                surface_high: 0x252a3d,
                border: 0x2e3348,
                selected_surface: 0x2a3049,
                selected_border: 0x7178e6,
                text: 0xedeff5,
                codex_icon: 0xffffff,
                claude_icon: 0xd97757,
                muted: 0xb0b6c9,
                faint: 0x959cb2,
                accent: 0x8b8ff8,
                accent_ink: 0xa9adff,
                accent_hover: 0xa6a9ff,
                accent_text: 0x0e1016,
                rule: 0x2a2e42,
                working: 0x63d8a8,
                waiting: 0xf0c06a,
                done: 0xa9adff,
                danger: 0xff94a2,
                danger_surface: 0x2c1520,
                diff_added: 0x173328,
                diff_removed: 0x3b1d28,
                selection: 0x8b8ff840,
                backdrop: 0x05060ab3,
                chrome: 0x1e2236,
                composer: 0x252a42,
                glass: Some(Glass {
                    wash_from: 0x1d1b4d,
                    wash_to: 0x0a2229,
                    glow: 0x6a2e8059,
                    pane: 0x2a2f528c,
                    composer: 0x30365cc7,
                    edge: 0xffffff24,
                }),
            },
            Self::ModernLight => ThemeColors {
                background: 0xeef0f9,
                sidebar: 0xf3f4fb,
                surface: 0xf8f9fd,
                surface_high: 0xeceef7,
                border: 0xd3d7e6,
                selected_surface: 0xdfe2f4,
                selected_border: 0x6366e0,
                text: 0x1b1e2c,
                codex_icon: 0x1b1e2c,
                claude_icon: 0xb85c3f,
                muted: 0x4b5165,
                faint: 0x5a6074,
                accent: 0x5257d9,
                accent_ink: 0x4045bb,
                accent_hover: 0x4347c4,
                accent_text: 0xffffff,
                rule: 0xdadde9,
                working: 0x17603f,
                waiting: 0x6c490a,
                done: 0x4045bb,
                danger: 0x97203a,
                danger_surface: 0xfbe9ed,
                diff_added: 0xdff2e6,
                diff_removed: 0xf8e1e6,
                selection: 0x5257d933,
                backdrop: 0x1b1e2c66,
                chrome: 0xf3f4fb,
                composer: 0xffffff,
                glass: Some(Glass {
                    wash_from: 0xdcdcfb,
                    wash_to: 0xd3ebf3,
                    glow: 0xf7d6ee99,
                    pane: 0xffffff8c,
                    composer: 0xffffffd0,
                    edge: 0x1b1e2c1f,
                }),
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
