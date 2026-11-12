//! Fills for the themes drawn as frosted glass ([`Glass`]). Every helper falls
//! back to the theme's opaque role, so callers draw the same way in any theme.

use gpui::{Background, Rgba, linear_color_stop, linear_gradient, rgb, rgba, transparent_black};
use xd_desktop::theme::{Glass, ThemeColors};

/// Top left to bottom right, as GPUI measures gradient angles.
const WASH_ANGLE: f32 = 135.0;
/// From the top right corner toward the bottom left.
const GLOW_ANGLE: f32 = 225.0;
/// How far across the window the glow reaches before it has faded out.
const GLOW_REACH: f32 = 0.6;

/// What the window paints behind everything: the glass wash, or the canvas.
pub(crate) fn window(colors: ThemeColors) -> Background {
    match colors.glass {
        Some(glass) => wash(glass),
        None => rgb(colors.background).into(),
    }
}

/// The corner glow over the wash, for a layer spanning the window just above
/// its own paint.
pub(crate) fn glow(colors: ThemeColors) -> Option<Background> {
    colors.glass.map(|glass| {
        linear_gradient(
            GLOW_ANGLE,
            linear_color_stop(rgba(glass.glow), 0.0),
            linear_color_stop(rgba(glass.glow & !0xff), GLOW_REACH),
        )
    })
}

/// A page canvas, such as the chat or a project's page. Under glass it is clear,
/// so the window's wash shows.
pub(crate) fn canvas(colors: ThemeColors) -> Background {
    match colors.glass {
        Some(_) => transparent_black().into(),
        None => rgb(colors.background).into(),
    }
}

/// A pane over the canvas, such as the sidebar or a title bar, whose opaque
/// color is `solid`.
pub(crate) fn pane(colors: ThemeColors, solid: u32) -> Background {
    match colors.glass {
        Some(glass) => rgba(glass.pane).into(),
        None => rgb(solid).into(),
    }
}

/// The message composer.
pub(crate) fn composer(colors: ThemeColors) -> Background {
    match colors.glass {
        Some(glass) => rgba(glass.composer).into(),
        None => rgb(colors.composer).into(),
    }
}

/// The hairline along a pane's edge: the glass edge, or the border role.
pub(crate) fn edge(colors: ThemeColors) -> Rgba {
    match colors.glass {
        Some(glass) => rgba(glass.edge),
        None => rgb(colors.border),
    }
}

fn wash(glass: Glass) -> Background {
    linear_gradient(
        WASH_ANGLE,
        linear_color_stop(rgb(glass.wash_from), 0.0),
        linear_color_stop(rgb(glass.wash_to), 1.0),
    )
}
