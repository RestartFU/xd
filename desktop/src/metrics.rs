//! The sizes the interface is drawn in, each named by what it is for, and
//! the one scale they are all drawn at.
//!
//! Text sizes, corners and control heights across the UI come from here and
//! not from numbers written where they are used, so one role is one size.
//!
//! # Interface size
//!
//! Every length in the UI is written in logical pixels: what it measures at
//! 100 %. [`px`] turns one into the window's own pixels at the size chosen in
//! Settings › Appearance, and [`logical`] turns a measurement the window
//! hands back (a pointer position, an element's bounds, a scroll offset)
//! into logical pixels again. So an `f32` length is always logical and a
//! [`Pixels`] is always the window's, and the two convert only through
//! those functions. Layout arithmetic, breakpoints and saved widths are all
//! in logical pixels and need no thought for the scale.
//!
//! GPUI's own helpers (`px_2`, `gap_3`, `text_sm`, a default line height)
//! are in rems, which the root view sizes with the same scale. Hairline
//! borders stay one pixel of the window, so they stay sharp.
//!
//! [`window_px`] is for the few lengths that belong to the window and not
//! to the interface: its own size on screen, and the room the platform's
//! window buttons take.

use std::cell::Cell;

use gpui::Pixels;

/// The sizes Settings › Appearance › Interface size offers, in percent.
pub(crate) const INTERFACE_SIZES: [u16; 5] = [90, 100, 115, 130, 150];
pub(crate) const DEFAULT_INTERFACE_SIZE: u16 = 100;

thread_local! {
    /// The scale the interface is drawn at: the chosen Interface size over
    /// a hundred. Everything draws on one thread. It changes only between
    /// frames, when the setting does.
    static INTERFACE_SCALE: Cell<f32> = const { Cell::new(1.0) };
}

/// The offered size nearest to `percent`: a settings file edited by hand
/// cannot make the interface too small to find the setting again.
pub(crate) fn nearest_interface_size(percent: u16) -> u16 {
    INTERFACE_SIZES
        .into_iter()
        .min_by_key(|size| size.abs_diff(percent))
        .unwrap_or(DEFAULT_INTERFACE_SIZE)
}

/// The size one step up or down from `percent`, or `percent` itself at
/// either end.
pub(crate) fn interface_size_step(percent: u16, larger: bool) -> u16 {
    let at = INTERFACE_SIZES
        .iter()
        .position(|size| *size == nearest_interface_size(percent))
        .unwrap_or(1);
    let next = if larger {
        (at + 1).min(INTERFACE_SIZES.len() - 1)
    } else {
        at.saturating_sub(1)
    };
    INTERFACE_SIZES[next]
}

/// Draws the interface at `percent` from the next frame on.
pub(crate) fn set_interface_size(percent: u16) {
    let scale = f32::from(nearest_interface_size(percent)) / 100.0;
    INTERFACE_SCALE.with(|current| current.set(scale));
}

/// The scale the interface is drawn at: 1 at 100 %.
pub(crate) fn interface_scale() -> f32 {
    INTERFACE_SCALE.with(Cell::get)
}

/// A logical length in the window's pixels, at the chosen Interface size.
pub(crate) fn px(length: f32) -> Pixels {
    gpui::px(length * interface_scale())
}

/// A text size in the window's pixels: the role's size at the Interface
/// size, and never smaller than a caption at 100 %. At 90 % captions and
/// labels both draw at 11 window pixels, the floor DESIGN.md sets.
pub(crate) fn text_px(size: f32) -> Pixels {
    gpui::px((size * interface_scale()).max(TEXT_CAPTION))
}

/// The side of a pointer target in the window's pixels: `size` at the
/// Interface size, and never under [`HIT_MIN`] window pixels.
pub(crate) fn hit_px(size: f32) -> Pixels {
    gpui::px((size * interface_scale()).max(HIT_MIN))
}

/// A length the window measured, in logical pixels.
pub(crate) fn logical(pixels: Pixels) -> f32 {
    f32::from(pixels) / interface_scale()
}

/// A length in the window's own pixels whatever the Interface size: the
/// window's size on screen, and what the platform draws in it.
pub(crate) fn window_px(length: f32) -> Pixels {
    gpui::px(length)
}

/// The size of a rem, which GPUI's own spacing and type helpers are in.
pub(crate) const REM: f32 = 16.0;

/// The smallest text: a count, a state or a stamp at the end of a row, and
/// the words in a pill. Nothing is set smaller.
pub(crate) const TEXT_CAPTION: f32 = 11.0;
/// Text that explains or annotates: notes, hints, tooltips, section labels,
/// and the chips of a header.
pub(crate) const TEXT_LABEL: f32 = 12.0;
/// The words on a control and the titles of rows: buttons, fields, menu
/// rows, tabs, session and project names, and the message being written.
pub(crate) const TEXT_CONTROL: f32 = 13.0;
/// Prose that is read: the transcript and what a dialog says.
pub(crate) const TEXT_BODY: f32 = 14.0;
/// Code and output in the monospace face: code blocks, diffs, the terminal
/// and the file editor. Only monospace text takes this size; a path or a
/// branch set inside a label takes the label's.
pub(crate) const TEXT_MONO: f32 = 12.5;
/// The heading of a dialog, a panel or a section, and a third-level heading
/// in prose.
pub(crate) const TEXT_HEADING: f32 = 16.0;
/// A second-level heading in prose, between [`TEXT_HEADING`] and
/// [`TEXT_TITLE`].
pub(crate) const TEXT_PROSE_H2: f32 = 19.0;
/// The title of a page, a figure that leads a panel, and a first-level
/// heading in prose.
pub(crate) const TEXT_TITLE: f32 = 22.0;
/// The one large line of a page: the heading of an empty chat and of a
/// Settings section.
pub(crate) const TEXT_DISPLAY: f32 = 28.0;
/// The total that leads the Usage page.
pub(crate) const TEXT_FIGURE: f32 = 36.0;

/// Corners of a control: buttons, icon buttons, fields, menu rows, tooltips.
pub(crate) const RADIUS_CONTROL: f32 = 6.0;
/// Corners of a tab and of a row that is picked like one.
pub(crate) const RADIUS_TAB: f32 = 7.0;
/// Corners of a panel: cards, menus, code blocks, image tiles.
pub(crate) const RADIUS_PANEL: f32 = 8.0;
/// Corners of a dialog.
pub(crate) const DIALOG_RADIUS: f32 = 10.0;
/// Corners of the composer, of the messages sent from it, and of the Usage
/// page's sheet.
pub(crate) const RADIUS_COMPOSER: f32 = 16.0;

/// The height of a control: a button, a field, a tab.
pub(crate) const CONTROL_HEIGHT: f32 = 30.0;
/// The height of a control set into a toolbar or a row, and the side of an
/// icon button.
pub(crate) const CONTROL_HEIGHT_COMPACT: f32 = 28.0;
/// The least a pointer target measures on either side. The glyph inside may
/// be smaller.
pub(crate) const HIT_MIN: f32 = 24.0;
