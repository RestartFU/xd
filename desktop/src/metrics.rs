//! The sizes the interface is drawn in, each named by what it is for. Text
//! sizes, corners and control heights across the UI come from here and not
//! from numbers written where they are used, so one role is one size.

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
