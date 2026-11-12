use serde::{Deserialize, Serialize};

/// A complete set of renderer-facing colors encoded as `0xRRGGBB` values.
///
/// Keeping the roles semantic lets GPUI views change theme without knowing
/// which concrete color belongs to a background, divider, or text treatment.
///
/// Both themes are frosted glass in the manner of macOS, drawn inside an opaque
/// window: Dark is black glass, Light frosted white over a cool grey, both with
/// an indigo accent and signal colors reserved for session state. See [`Glass`].
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
    /// The frosted glass the window, panes and composer draw with. The opaque
    /// roles above stay complete, so anything that does not draw glass (cards,
    /// menus, dialogs, previews) still looks right.
    pub glass: Glass,
}

/// Glass drawn inside the app; the window itself stays opaque, so nothing
/// outside it ever shows through.
///
/// The window paints one flat backdrop. Page canvases (the chat, a project's
/// page, the session column) leave it showing, and the sidebar, title bars, tab
/// rows and composer are translucent panes over it, set apart by a hairline
/// edge. Reading surfaces such as the terminal, code blocks, cards, menus and
/// dialogs keep their opaque roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glass {
    /// What the window paints behind everything.
    pub backdrop: u32,
    /// The sidebar, title bars and tab rows, as `0xRRGGBBAA`.
    pub pane: u32,
    /// The message composer, as `0xRRGGBBAA`.
    pub composer: u32,
    /// The hairline along a pane's edge, as `0xRRGGBBAA`: light on the dark
    /// backdrop, dark on the light one.
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
                background: 0x0b0b0b,
                sidebar: 0x0f0f0f,
                surface: 0x111111,
                surface_high: 0x1b1b1b,
                border: 0x2a2a2a,
                selected_surface: 0x222222,
                selected_border: 0x7178e6,
                text: 0xf2f2f2,
                codex_icon: 0xffffff,
                claude_icon: 0xd97757,
                muted: 0xadadad,
                faint: 0x929292,
                accent: 0x8b8ff8,
                accent_ink: 0xa9adff,
                accent_hover: 0xa6a9ff,
                accent_text: 0x0e1016,
                rule: 0x262626,
                working: 0x63d8a8,
                waiting: 0xf0c06a,
                done: 0xa9adff,
                danger: 0xff94a2,
                danger_surface: 0x2a1416,
                diff_added: 0x142b20,
                diff_removed: 0x351a20,
                selection: 0x8b8ff840,
                backdrop: 0x000000b3,
                chrome: 0x0f0f0f,
                composer: 0x161616,
                glass: Glass {
                    backdrop: 0x0b0b0b,
                    pane: 0xffffff04,
                    composer: 0x161616d9,
                    edge: 0xffffff1f,
                },
            },
            Self::Light => ThemeColors {
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
                glass: Glass {
                    backdrop: 0xe4e7f4,
                    pane: 0xffffff8c,
                    composer: 0xffffffd0,
                    edge: 0x1b1e2c1f,
                },
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
