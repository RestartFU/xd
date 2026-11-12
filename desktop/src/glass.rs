//! Fills for the frosted glass every theme draws with ([`xd_desktop::theme::Glass`]).

use gpui::{Background, Rgba, rgb, rgba, transparent_black};
use xd_desktop::theme::ThemeColors;

/// What the window paints behind everything.
pub(crate) fn window(colors: ThemeColors) -> Background {
    rgb(colors.glass.backdrop).into()
}

/// A page canvas, such as the chat or a project's page: clear, so the window's
/// backdrop shows.
pub(crate) fn canvas() -> Background {
    transparent_black().into()
}

/// A pane over the canvas, such as the sidebar or a title bar.
pub(crate) fn pane(colors: ThemeColors) -> Background {
    rgba(colors.glass.pane).into()
}

/// The message composer.
pub(crate) fn composer(colors: ThemeColors) -> Background {
    rgba(colors.glass.composer).into()
}

/// The hairline along a pane's edge.
pub(crate) fn edge(colors: ThemeColors) -> Rgba {
    rgba(colors.glass.edge)
}
