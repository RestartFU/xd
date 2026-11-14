use serde::{Deserialize, Serialize};

use xd_terminal::TerminalPalette;

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
    /// A quiet hairline: between panes, around cards, menus and dialogs.
    pub border: u32,
    /// The edge that shows where a control is: a field's frame and a
    /// secondary button's. It reads at 3:1 or more on every ground a control
    /// sits on; [`ThemeColors::border`] stays quiet for decoration.
    pub control_border: u32,
    pub selected_surface: u32,
    pub selected_border: u32,
    pub text: u32,
    pub muted: u32,
    /// Assistant brand marks, independent of selection and activity state.
    pub codex_icon: u32,
    pub claude_icon: u32,
    /// Counts, hints, and timestamps; never body copy.
    pub faint: u32,
    /// The words and icon of a control that cannot be used now: at 4.5:1 or
    /// more on every ground, and, where a theme leaves room for it, 1.5:1 or
    /// more apart from [`ThemeColors::muted`], the ink of a quiet control
    /// that can. Light leaves none: an ink that far lighter than its muted
    /// falls under 4.5:1 on its paler grounds. In both themes a disabled
    /// control also has a dashed edge and no fill, which no control that can
    /// be used has.
    pub disabled: u32,
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
    /// Source highlighting in the file editor.
    pub syntax: Syntax,
    /// Usage's bars and charts.
    pub chart: ChartInk,
    /// The thumb that shows how far a scroll area has scrolled, as
    /// `0xRRGGBBAA`: at 3:1 or more over every ground it is drawn on.
    pub scroll_thumb: u32,
    /// The wash under an icon button on a tinted banner, such as the error
    /// banner's close button, as `0xRRGGBBAA` of the banner's ink.
    pub banner_wash: u32,
    /// A pill's fill: a quiet tint of its ink, as the alpha that tint takes.
    pub pill_alpha: u8,
    /// The faintest a pulsing working dot gets, as an alpha: the working ink
    /// at this strength still reads at 3:1 on every ground.
    pub pulse_floor: u8,
    /// The full-window image viewer and the buttons laid over pictures.
    pub viewer: ViewerInk,
}

/// What the image viewer draws with, and the buttons laid over a picture.
/// A picture is seen best on black whatever the theme, so both themes keep
/// the viewer dark; it is still named here, with the other roles. Each is
/// `0xRRGGBBAA` but `ink`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewerInk {
    /// What the viewer lays over the window.
    pub backdrop: u32,
    /// A button over a picture, under the pointer, and its edge.
    pub button: u32,
    pub button_hover: u32,
    pub button_edge: u32,
    /// The viewer's side buttons under the pointer.
    pub hover: u32,
    /// Icons over a picture, opaque `0xRRGGBB`.
    pub ink: u32,
    /// The picture's name under it.
    pub caption: u32,
}

/// The viewer's ink, which both themes share.
const VIEWER_INK: ViewerInk = ViewerInk {
    backdrop: 0x000000bf,
    button: 0x000000a6,
    button_hover: 0x000000cc,
    button_edge: 0xffffff33,
    hover: 0xffffff1a,
    ink: 0xffffff,
    caption: 0xffffffcc,
};

/// Usage's figures in shades of the text ink over the page, so colour stays
/// reserved for status. Each shade is opaque and already mixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartInk {
    /// The empty part of a bar.
    pub track: u32,
    /// Four steps from faint to strong, for the parts of a share bar and for
    /// providers told apart by shade; the text ink is the fifth.
    pub shades: [u32; 4],
    /// The wash under a chart's line, as `0xRRGGBBAA`: Claude's line, and
    /// every other line, which is drawn in the text ink.
    pub claude_area: u32,
    pub area: u32,
}

/// The colours source code is highlighted with; anything else is `text`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    pub keyword: u32,
    pub string: u32,
    pub comment: u32,
    pub number: u32,
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

    /// The ANSI colours the terminal draws with on this theme's ground.
    pub const fn terminal_palette(self) -> TerminalPalette {
        match self {
            Self::Dark => TerminalPalette::Dark,
            Self::Light => TerminalPalette::Light,
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
                control_border: 0x6e6e6e,
                selected_surface: 0x222222,
                selected_border: 0x7178e6,
                text: 0xf2f2f2,
                codex_icon: 0xffffff,
                claude_icon: 0xd97757,
                muted: 0xadadad,
                faint: 0x929292,
                disabled: 0x898989,
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
                syntax: Syntax {
                    keyword: 0xc7a0e8,
                    string: 0xa9cf8f,
                    comment: 0x8b8a85,
                    number: 0xe9a07a,
                },
                chart: ChartInk {
                    track: 0x2f2f2f,
                    shades: [0x505050, 0x717171, 0x919191, 0xb1b1b1],
                    claude_area: 0xd977571f,
                    area: 0xf2f2f21f,
                },
                scroll_thumb: 0xffffff5c,
                banner_wash: 0xff94a224,
                pill_alpha: 0x24,
                pulse_floor: 0x78,
                viewer: VIEWER_INK,
            },
            Self::Light => ThemeColors {
                background: 0xeef0f9,
                sidebar: 0xf3f4fb,
                surface: 0xf8f9fd,
                surface_high: 0xeceef7,
                border: 0xd3d7e6,
                control_border: 0x7a7f92,
                selected_surface: 0xdfe2f4,
                selected_border: 0x6366e0,
                text: 0x1b1e2c,
                codex_icon: 0x1b1e2c,
                claude_icon: 0xb85c3f,
                muted: 0x4b5165,
                faint: 0x5a6074,
                disabled: 0x5a6074,
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
                syntax: Syntax {
                    keyword: 0x7a3fa8,
                    string: 0x3d6d22,
                    comment: 0x6e6d67,
                    number: 0xa0461c,
                },
                chart: ChartInk {
                    track: 0xd5d6de,
                    shades: [0xafb1bb, 0x91949f, 0x747682, 0x565965],
                    claude_area: 0xb85c3f1f,
                    area: 0x1b1e2c1f,
                },
                scroll_thumb: 0x1b1e2c84,
                banner_wash: 0x97203a24,
                pill_alpha: 0x24,
                pulse_floor: 0xac,
                viewer: VIEWER_INK,
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

    #[test]
    fn light_terminal_colours_read_on_the_light_terminal_ground() {
        let light = ThemePreset::Light;
        let ground = light.colors().background;
        for (index, color) in light.terminal_palette().colors().iter().enumerate() {
            let ratio = xd_terminal::contrast_ratio(*color, ground);
            assert!(ratio >= 4.5, "colour {index} #{color:06x} is {ratio:.2}:1");
        }
    }

    #[test]
    fn dark_terminal_colours_read_on_every_dark_ground() {
        let dark = ThemePreset::Dark;
        let colors = dark.colors();
        // Black is a ground for text set on it, not ink for the dark ground.
        for (index, color) in dark.terminal_palette().colors().iter().enumerate().skip(1) {
            for ground in [colors.background, colors.surface, colors.surface_high] {
                let ratio = xd_terminal::contrast_ratio(*color, ground);
                assert!(
                    ratio >= 4.5,
                    "colour {index} #{color:06x} is {ratio:.2}:1 on #{ground:06x}"
                );
            }
        }
    }

    #[test]
    fn the_terminal_inverts_with_the_themes_own_text_and_ground() {
        for theme in ThemePreset::ALL {
            let colors = theme.colors();
            let palette = theme.terminal_palette();
            assert_eq!(palette.foreground(), colors.text, "{theme:?}");
            assert_eq!(palette.background(), colors.background, "{theme:?}");
        }
    }

    #[test]
    fn syntax_colours_read_on_the_editor_ground_in_both_themes() {
        for theme in ThemePreset::ALL {
            let colors = theme.colors();
            let Syntax {
                keyword,
                string,
                comment,
                number,
            } = colors.syntax;
            for color in [keyword, string, comment, number] {
                let ratio = xd_terminal::contrast_ratio(color, colors.background);
                assert!(ratio >= 4.5, "{theme:?} #{color:06x} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn each_theme_draws_the_terminal_with_its_own_palette() {
        assert_eq!(ThemePreset::Dark.terminal_palette(), TerminalPalette::Dark);
        assert_eq!(
            ThemePreset::Light.terminal_palette(),
            TerminalPalette::Light
        );
        assert_ne!(
            TerminalPalette::Dark.colors(),
            TerminalPalette::Light.colors()
        );
    }
}
