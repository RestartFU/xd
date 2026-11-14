//! Mouse text selection for rendered text.
//!
//! GPUI paints text; it has no notion of selecting it. This wraps an already
//! laid-out text element so dragging across it highlights a range, double-click
//! selects a word, and Ctrl+C copies that range. Markdown lays each block out
//! separately, so every selectable carries its range in one shared document.

use std::ops::Range;
use std::rc::Rc;

use crate::metrics::px;
use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, Global,
    GlobalElementId, Hitbox, HitboxBehavior, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, TextLayout, Window,
    fill, point, rgba,
};

/// The theme's selection fill, as `0xRRGGBBAA`: the one `ThemeColors::selection`
/// the editors use, so every selection in the app is the same colour. The
/// window sets it each time the theme changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionColor(u32);

impl Global for SelectionColor {}

impl SelectionColor {
    pub fn set(color: u32, cx: &mut App) {
        if cx.try_global::<Self>() != Some(&Self(color)) {
            cx.set_global(Self(color));
        }
    }

    /// Text painted before a window has set the theme uses the default one's.
    fn current(cx: &App) -> u32 {
        cx.try_global::<Self>().map_or_else(
            || crate::settings::ThemePreset::default().colors().selection,
            |color| color.0,
        )
    }
}

/// Reads the text a selection covers in a document drawn a line at a time:
/// the range is in the document's offsets, where line `n` starts at
/// `n * LINE_STRIDE`.
pub type LineSource = Rc<dyn Fn(Range<usize>, &App) -> Option<String>>;

/// How far apart two lines of a document drawn a line at a time start in its
/// offsets: more than any line holds.
pub const LINE_STRIDE: usize = 1 << 20;

/// One row of a document drawn a line at a time, such as a terminal's
/// scrollback, of which only the rows in view are drawn: whether it is the
/// first or last of those, and where the selection reads its lines from.
#[derive(Clone)]
pub struct LineRow {
    pub first: bool,
    pub last: bool,
    pub source: LineSource,
}

impl LineRow {
    /// Whether this row places the selection's head for a pointer at height
    /// `y`, kept inside the view: the row at that height, or the first or
    /// last in view for a height past it.
    fn owns(&self, y: Pixels, top: Pixels, bottom: Pixels) -> bool {
        (self.first || y >= top) && (self.last || y < bottom)
    }
}

/// The one selection in the window, keyed by the document that owns it.
#[derive(Clone, Default)]
pub struct TextSelection {
    block: Option<u64>,
    text: SharedString,
    anchor: usize,
    head: usize,
    dragging: bool,
    pressed_link: Option<(u64, usize)>,
    /// For a document drawn a line at a time, where its text is read from.
    lines: Option<LineSource>,
}

impl Global for TextSelection {}

impl TextSelection {
    fn range(&self) -> Range<usize> {
        if self.anchor <= self.head {
            self.anchor..self.head
        } else {
            self.head..self.anchor
        }
    }

    /// The selected text, when a selection covers anything.
    pub fn selected(cx: &App) -> Option<String> {
        let selection = cx.try_global::<Self>()?;
        if let Some(lines) = &selection.lines {
            return lines(selection.range(), cx).filter(|text| !text.is_empty());
        }
        selection
            .text
            .get(selection.range())
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }

    pub fn clear(cx: &mut App) {
        if cx
            .try_global::<Self>()
            .is_some_and(|selection| selection.block.is_some())
        {
            cx.set_global(Self::default());
        }
    }

    fn owns(cx: &App, document: u64) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|selection| selection.block == Some(document))
    }
}

/// Makes one laid-out block selectable inside a shared document. The range is
/// the exact slice of `text` painted by `layout`.
pub fn selectable_in_document(
    block: u64,
    document: u64,
    text: SharedString,
    range: Range<usize>,
    layout: TextLayout,
    element: impl IntoElement,
) -> Selectable {
    Selectable {
        block,
        document,
        document_text: Some(text),
        document_range: Some(range),
        layout,
        element: Some(element.into_any_element()),
        links: Vec::new(),
        link_listener: None,
        selection_scroll: None,
        line: None,
    }
}

/// Makes line `index` of a document drawn a line at a time selectable: its
/// offsets start at `index * LINE_STRIDE`, and a selection across lines
/// reads them from the row's source when it is copied.
#[allow(clippy::too_many_arguments)]
pub fn selectable_line_in_document(
    block: u64,
    document: u64,
    index: usize,
    text: SharedString,
    layout: TextLayout,
    element: impl IntoElement,
    links: Vec<Range<usize>>,
    listener: impl Fn(usize, &mut Window, &mut App) + 'static,
    row: LineRow,
) -> Selectable {
    let start = index * LINE_STRIDE;
    Selectable {
        block,
        document,
        document_range: Some(start..start + text.len()),
        document_text: Some(text),
        layout,
        element: Some(element.into_any_element()),
        links,
        link_listener: Some(Box::new(listener)),
        selection_scroll: None,
        line: Some(row),
    }
}

/// The text from `range` of a document drawn a line at a time, whose line
/// `n` is `line(n)`: the lines joined by newlines, as a terminal copies them.
pub fn lines_text(range: Range<usize>, mut line: impl FnMut(usize) -> Option<String>) -> String {
    let (first, last) = (range.start / LINE_STRIDE, range.end / LINE_STRIDE);
    let mut text = String::new();
    for index in first..=last {
        let Some(whole) = line(index) else {
            break;
        };
        let clamp = |offset: usize| {
            let mut offset = offset.min(whole.len());
            while !whole.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let start = if index == first {
            clamp(range.start % LINE_STRIDE)
        } else {
            0
        };
        let end = if index == last {
            clamp(range.end % LINE_STRIDE)
        } else {
            whole.len()
        };
        if index > first {
            text.push('\n');
        }
        text.push_str(&whole[start..end.max(start)]);
    }
    text
}

#[allow(clippy::too_many_arguments)]
pub fn selectable_links_in_document(
    block: u64,
    document: u64,
    text: SharedString,
    range: Range<usize>,
    layout: TextLayout,
    element: impl IntoElement,
    links: Vec<Range<usize>>,
    listener: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Selectable {
    Selectable {
        block,
        document,
        document_text: Some(text),
        document_range: Some(range),
        layout,
        element: Some(element.into_any_element()),
        links,
        link_listener: Some(Box::new(listener)),
        selection_scroll: None,
        line: None,
    }
}

type LinkListener = Box<dyn Fn(usize, &mut Window, &mut App)>;

pub struct Selectable {
    block: u64,
    document: u64,
    document_text: Option<SharedString>,
    document_range: Option<Range<usize>>,
    layout: TextLayout,
    element: Option<AnyElement>,
    links: Vec<Range<usize>>,
    link_listener: Option<LinkListener>,
    selection_scroll: Option<ScrollHandle>,
    line: Option<LineRow>,
}

impl Selectable {
    pub fn with_selection_scroll(mut self, scroll: ScrollHandle) -> Self {
        self.selection_scroll = Some(scroll);
        self
    }

    fn paint_selection(&self, window: &mut Window, cx: &App) {
        if !TextSelection::owns(cx, self.document) {
            return;
        }
        if self.line.is_some() {
            self.paint_line_selection(window, cx);
            return;
        }
        let document_range = self
            .document_range
            .clone()
            .unwrap_or_else(|| 0..self.layout.len());
        let Some(range) = cx
            .try_global::<TextSelection>()
            .map(TextSelection::range)
            .and_then(|range| selection_range_in_block(&range, &document_range))
        else {
            return;
        };
        let (Some(start), Some(end)) = (
            self.layout.position_for_index(range.start),
            self.layout.position_for_index(range.end),
        ) else {
            return;
        };
        let bounds = self.layout.bounds();
        let height = self.layout.line_height();
        // Wrapped text selects as a first partial line, a full-width block, and
        // a last partial line — the shape a reader expects.
        let color = SelectionColor::current(cx);
        if start.y == end.y {
            paint_row(window, start, point(end.x, start.y + height), color);
            return;
        }
        paint_row(
            window,
            start,
            point(bounds.right(), start.y + height),
            color,
        );
        if end.y > start.y + height {
            paint_row(
                window,
                point(bounds.left(), start.y + height),
                point(bounds.right(), end.y),
                color,
            );
        }
        paint_row(
            window,
            point(bounds.left(), end.y),
            point(end.x, end.y + height),
            color,
        );
    }

    /// A row of a document drawn a line at a time selects as a line of
    /// the whole would: from the selection's start, and on to the row's
    /// right edge when the selection goes on past its end, as the newline
    /// that ends it would be selected.
    fn paint_line_selection(&self, window: &mut Window, cx: &App) {
        let Some(row) = self.document_range.clone() else {
            return;
        };
        let Some(selection) = cx.try_global::<TextSelection>().map(TextSelection::range) else {
            return;
        };
        if selection.is_empty() || selection.end <= row.start || selection.start > row.end {
            return;
        }
        let start = selection.start.max(row.start) - row.start;
        let Some(from) = self.layout.position_for_index(start) else {
            return;
        };
        let bounds = self.layout.bounds();
        let to = if selection.end > row.end {
            bounds.right()
        } else {
            match self.layout.position_for_index(selection.end - row.start) {
                Some(end) => end.x,
                None => return,
            }
        };
        paint_row(
            window,
            from,
            point(to, from.y + self.layout.line_height()),
            SelectionColor::current(cx),
        );
    }

    fn listen(
        &self,
        hitbox: &Hitbox,
        link_listener: Option<LinkListener>,
        window: &mut Window,
        cx: &App,
    ) {
        let block = self.block;
        let document = self.document;
        let layout = self.layout.clone();
        let layout_text = layout.text();
        let document_text = self
            .document_text
            .clone()
            .unwrap_or_else(|| layout_text.clone().into());
        let document_range = self.document_range.clone().unwrap_or(0..layout_text.len());
        let links = self.links.clone();
        let lines = self.line.as_ref().map(|line| line.source.clone());
        let pressed = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !pressed.is_hovered(window)
            {
                return;
            }
            let index = index_at(&layout, event.position);
            let text = layout.text();
            let range = range_for_click(&text, index, event.click_count);
            cx.set_global(TextSelection {
                block: Some(document),
                text: document_text.clone(),
                anchor: document_range.start + range.start,
                head: document_range.start + range.end,
                // A small pointer movement between the two presses should not
                // collapse the word that the second press just selected.
                dragging: event.click_count == 1,
                pressed_link: links
                    .iter()
                    .position(|range| range.contains(&index))
                    .map(|link| (block, link)),
                lines: lines.clone(),
            });
            window.refresh();
        });

        let document_range = self
            .document_range
            .clone()
            .unwrap_or_else(|| 0..self.layout.len());
        let layout = self.layout.clone();
        let moved = hitbox.clone();
        let selection_scroll = self.selection_scroll.clone();
        let line = self.line.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !event.dragging() {
                return;
            }
            if !cx
                .try_global::<TextSelection>()
                .is_some_and(|selection| selection.dragging && selection.block == Some(document))
            {
                return;
            }

            let mut scrolled = false;
            let mut position = event.position;
            if let Some(scroll) = selection_scroll.as_ref() {
                let bounds = scroll.bounds();
                // The rows in view share the drag: only the one at the
                // pointer's height, kept inside the view, scrolls and places
                // the head.
                if let Some(line) = &line {
                    let y = clamp_into(event.position.y, bounds.top(), bounds.bottom());
                    if !line.owns(y, moved.top(), moved.bottom()) {
                        return;
                    }
                    position.y = y;
                }
                if let Some(y) = selection_edge_scroll(
                    scroll.offset().y,
                    scroll.max_offset().height,
                    event.position.y,
                    bounds.top(),
                    bounds.bottom(),
                ) {
                    let mut offset = scroll.offset();
                    offset.y = y;
                    scroll.set_offset(offset);
                    scrolled = true;
                }
            } else if block != document
                && (event.position.y < moved.top() || event.position.y > moved.bottom())
            {
                // Every block of a shared document hears this move; only the
                // one under the pointer may place the head.
                return;
            }
            // A block that is its whole document keeps following a drag that
            // leaves it: GPUI reports index 0 above the text and its end below,
            // so overshooting the block selects through to its edge.

            let index = document_range.start + index_at(&layout, position);
            let selection = cx.global_mut::<TextSelection>();
            if selection.head == index {
                if scrolled {
                    window.refresh();
                }
                return;
            }
            selection.head = index;
            window.refresh();
        });

        let layout = self.layout.clone();
        let links = self.links.clone();
        let released = hitbox.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !TextSelection::owns(cx, document)
            {
                return;
            }
            let pressed_link = {
                let selection = cx.global_mut::<TextSelection>();
                selection.dragging = false;
                match selection.pressed_link {
                    Some((pressed_block, link)) if pressed_block == block => {
                        selection.pressed_link = None;
                        Some(link)
                    }
                    _ => None,
                }
            };
            if released.is_hovered(window)
                && let (Some(link), Some(listener)) = (pressed_link, link_listener.as_ref())
                && links.get(link).is_some_and(|range| {
                    layout
                        .index_for_position(event.position)
                        .is_ok_and(|index| range.contains(&index))
                })
            {
                listener(link, window, cx);
            }
        });

        if let Some(scroll) = self.selection_scroll.clone() {
            let owner = self.line.as_ref().is_none_or(|line| {
                let bounds = scroll.bounds();
                let y = clamp_into(window.mouse_position().y, bounds.top(), bounds.bottom());
                line.owns(y, hitbox.top(), hitbox.bottom())
            });
            if owner {
                schedule_selection_auto_scroll(
                    document,
                    document_range,
                    self.layout.clone(),
                    scroll,
                    self.line.is_some(),
                    window,
                    cx,
                );
            }
        }
    }
}

fn schedule_selection_auto_scroll(
    document: u64,
    document_range: Range<usize>,
    layout: TextLayout,
    scroll: ScrollHandle,
    line: bool,
    window: &mut Window,
    cx: &App,
) {
    if !cx
        .try_global::<TextSelection>()
        .is_some_and(|selection| selection.dragging && selection.block == Some(document))
    {
        return;
    }
    let bounds = scroll.bounds();
    if selection_edge_scroll(
        scroll.offset().y,
        scroll.max_offset().height,
        window.mouse_position().y,
        bounds.top(),
        bounds.bottom(),
    )
    .is_none()
    {
        return;
    }

    window.on_next_frame(move |window, cx| {
        if !cx
            .try_global::<TextSelection>()
            .is_some_and(|selection| selection.dragging && selection.block == Some(document))
        {
            return;
        }
        let pointer = window.mouse_position();
        let bounds = scroll.bounds();
        let Some(y) = selection_edge_scroll(
            scroll.offset().y,
            scroll.max_offset().height,
            pointer.y,
            bounds.top(),
            bounds.bottom(),
        ) else {
            return;
        };

        let mut offset = scroll.offset();
        offset.y = y;
        scroll.set_offset(offset);
        let mut pointer = pointer;
        if line {
            pointer.y = clamp_into(pointer.y, bounds.top(), bounds.bottom());
        }
        let index = document_range.start + index_at(&layout, pointer);
        cx.global_mut::<TextSelection>().head = index;
        window.refresh();
    });
}

/// `y` kept inside the band from `top` to just above `bottom`.
fn clamp_into(y: Pixels, top: Pixels, bottom: Pixels) -> Pixels {
    y.max(top).min((bottom - px(1.)).max(top))
}

/// The selected portion painted by one block, translated back to local bytes.
fn selection_range_in_block(
    selection: &Range<usize>,
    block: &Range<usize>,
) -> Option<Range<usize>> {
    let start = selection.start.max(block.start);
    let end = selection.end.min(block.end);
    (start < end).then(|| start - block.start..end - block.start)
}

fn paint_row(window: &mut Window, start: Point<Pixels>, end: Point<Pixels>, color: u32) {
    if end.x <= start.x || end.y <= start.y {
        return;
    }
    window.paint_quad(fill(Bounds::from_corners(start, end), rgba(color)));
}

/// The byte offset a point lands on. Dragging past the text reports the nearest
/// offset, which is what carries a selection to the end of a line.
fn index_at(layout: &TextLayout, position: Point<Pixels>) -> usize {
    match layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    }
}

fn selection_edge_scroll(
    current: Pixels,
    maximum: Pixels,
    pointer: Pixels,
    top: Pixels,
    bottom: Pixels,
) -> Option<Pixels> {
    let step = px(19.);
    // Start while the pointer is still inside the viewport. A terminal can
    // touch a window edge, where the OS cannot report a pointer position past
    // the scroll bounds, making an outside-only trigger impossible to reach.
    let next = if pointer < top + step {
        (current + step).min(px(0.))
    } else if pointer > bottom - step {
        (current - step).max(-maximum)
    } else {
        return None;
    };
    (next != current).then_some(next)
}

fn cursor_for_hover(is_link: bool) -> CursorStyle {
    if is_link {
        CursorStyle::OpenHand
    } else {
        CursorStyle::IBeam
    }
}

/// The run a double-click selects. Words include Unicode letters and numbers
/// plus underscores; whitespace and punctuation each form their own runs, but
/// a newline never carries selection onto another visual line.
fn word_range(text: &str, index: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }

    let mut target = index.min(text.len());
    while target > 0 && !text.is_char_boundary(target) {
        target -= 1;
    }
    if target == text.len() {
        target = text
            .char_indices()
            .next_back()
            .map_or(0, |(offset, _)| offset);
    }

    let character = text[target..]
        .chars()
        .next()
        .expect("a clamped character boundary has a character");
    let class = character_class(character);
    let mut start = target;
    for (offset, character) in text[..target].char_indices().rev() {
        if character_class(character) != class {
            break;
        }
        start = offset;
    }

    let mut end = target + character.len_utf8();
    let following = end;
    for (offset, character) in text[following..].char_indices() {
        if character_class(character) != class {
            break;
        }
        end = following + offset + character.len_utf8();
    }
    start..end
}

fn range_for_click(text: &str, index: usize, click_count: usize) -> Range<usize> {
    match click_count {
        1 => index..index,
        2 => word_range(text, index),
        _ => line_range(text, index),
    }
}

/// The complete newline-delimited row under a pointer. Terminal snapshots
/// encode each visual row on its own line, so this is also the terminal's
/// triple-click selection unit.
fn line_range(text: &str, index: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }

    let mut target = index.min(text.len());
    while target > 0 && !text.is_char_boundary(target) {
        target -= 1;
    }
    let start = text[..target].rfind('\n').map_or(0, |newline| newline + 1);
    let mut end = text[target..]
        .find('\n')
        .map_or(text.len(), |newline| target + newline);
    if end > start && text.as_bytes().get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    start..end
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharacterClass {
    Word,
    Whitespace,
    Newline,
    Punctuation,
}

fn character_class(character: char) -> CharacterClass {
    match character {
        '\n' | '\r' => CharacterClass::Newline,
        '_' => CharacterClass::Word,
        character if character.is_alphanumeric() => CharacterClass::Word,
        character if character.is_whitespace() => CharacterClass::Whitespace,
        _ => CharacterClass::Punctuation,
    }
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut element = self
            .element
            .take()
            .expect("selectable text lays out exactly once");
        let layout_id = element.request_layout(window, cx);
        (layout_id, element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        element.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_selection(window, cx);
        let cursor = cursor_for_hover(
            self.links
                .iter()
                .any(|range| range.contains(&index_at(&self.layout, window.mouse_position()))),
        );
        window.set_cursor_style(cursor, hitbox);
        element.paint(window, cx);
        let link_listener = self.link_listener.take();
        self.listen(hitbox, link_listener, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, ListAlignment, ListState, Modifiers, Render, ScrollHandle, Styled, StyledText,
        TestAppContext, div, list, prelude::*, px,
    };

    #[gpui::test]
    fn the_selection_fill_follows_the_theme(cx: &mut TestAppContext) {
        use crate::settings::ThemePreset;
        cx.update(|cx| {
            assert_eq!(
                SelectionColor::current(cx),
                ThemePreset::Dark.colors().selection
            );
            let light = ThemePreset::Light.colors().selection;
            SelectionColor::set(light, cx);
            assert_eq!(SelectionColor::current(cx), light);
        });
    }

    #[test]
    fn hovering_a_link_uses_an_open_hand_cursor() {
        assert_eq!(cursor_for_hover(true), CursorStyle::OpenHand);
        assert_eq!(cursor_for_hover(false), CursorStyle::IBeam);
    }

    #[gpui::test]
    fn a_selectable_interactive_range_still_receives_click(cx: &mut TestAppContext) {
        struct SelectableLinkList {
            list: ListState,
        }

        impl Render for SelectableLinkList {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                list(self.list.clone(), move |_, _, _| {
                    let value: SharedString = "open link".into();
                    let text = StyledText::new(value.clone());
                    let layout = text.layout().clone();
                    selectable_links_in_document(
                        1,
                        1,
                        value,
                        0..9,
                        layout,
                        text,
                        vec![0..9],
                        |_, _, cx| cx.open_url("https://example.com"),
                    )
                    .into_any_element()
                })
                .size_full()
            }
        }

        let (_, cx) = cx.add_window_view(|_, _| SelectableLinkList {
            list: ListState::new(1, ListAlignment::Top, px(100.0)),
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            cx.set_global(TextSelection {
                block: Some(1),
                text: "open link".into(),
                anchor: 0,
                head: 9,
                dragging: false,
                pressed_link: None,
                lines: None,
            });
        });
        let link = point(px(10.0), px(10.0));
        cx.simulate_mouse_move(link, None, Modifiers::default());
        cx.simulate_mouse_down(link, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(link, MouseButton::Left, Modifiers::default());

        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
    }

    #[gpui::test]
    fn dragging_a_selection_above_a_scroller_reveals_earlier_lines(cx: &mut TestAppContext) {
        struct ScrollSelection {
            scroll: ScrollHandle,
        }

        impl Render for ScrollSelection {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let value: SharedString = (0..80)
                    .map(|line| format!("line {line:02}"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into();
                let text = StyledText::new(value.clone());
                let layout = text.layout().clone();
                let range = 0..value.len();

                div()
                    .id("selection-scroll-test")
                    .w(px(240.0))
                    .h(px(120.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        selectable_in_document(1, 1, value, range, layout, text)
                            .with_selection_scroll(self.scroll.clone()),
                    )
            }
        }

        let scroll = ScrollHandle::new();
        scroll.scroll_to_bottom();
        let (_, cx) = cx.add_window_view({
            let scroll = scroll.clone();
            move |_, _| ScrollSelection { scroll }
        });
        cx.run_until_parked();
        let bottom = scroll.offset().y;
        assert!(bottom < px(0.0), "test content must overflow its viewport");

        cx.simulate_mouse_move(point(px(30.0), px(90.0)), None, Modifiers::default());
        cx.simulate_mouse_down(
            point(px(30.0), px(90.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(30.0), px(5.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(
            scroll.offset().y > bottom,
            "dragging above the viewport should scroll toward older output"
        );
    }

    #[gpui::test]
    fn dragging_past_the_bottom_of_the_text_selects_to_its_end(cx: &mut TestAppContext) {
        struct PaddedText;

        impl Render for PaddedText {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let value: SharedString = "first line\nsecond line".into();
                let text = StyledText::new(value.clone());
                let layout = text.layout().clone();
                let range = 0..value.len();
                div()
                    .w(px(240.0))
                    .p(px(20.0))
                    .child(selectable_in_document(1, 1, value, range, layout, text))
            }
        }

        let (_, cx) = cx.add_window_view(|_, _| PaddedText);
        cx.run_until_parked();

        let start = point(px(21.0), px(22.0));
        let below = point(px(30.0), px(200.0));
        cx.simulate_mouse_move(start, None, Modifiers::default());
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(below, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(below, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        let selected = cx.update(|_, cx| TextSelection::selected(cx));
        assert_eq!(selected.as_deref(), Some("first line\nsecond line"));
    }

    #[test]
    fn a_selection_across_lines_drawn_one_at_a_time_reads_them_joined() {
        let lines = ["first line", "", "third", "fourth line"];
        let line = |index: usize| lines.get(index).map(|line| line.to_string());
        let at = |line: usize, column: usize| line * LINE_STRIDE + column;
        assert_eq!(lines_text(at(0, 6)..at(2, 3), line), "line\n\nthi");
        assert_eq!(lines_text(at(3, 0)..at(3, 6), line), "fourth");
        // Past a line's end, as a drag to the right of it reaches.
        assert_eq!(lines_text(at(2, 2)..at(3, 99), line), "ird\nfourth line");
        assert_eq!(lines_text(at(3, 7)..at(9, 0), line), "line");
    }

    /// The rows a list draws in view, each a line of one document: a drag
    /// from one to another selects across them, and copies the lines.
    #[gpui::test]
    fn a_drag_across_rows_drawn_in_view_selects_the_lines_between(cx: &mut TestAppContext) {
        struct Rows {
            scroll: gpui::UniformListScrollHandle,
        }

        fn row_text(index: usize) -> String {
            format!("row {index:03}")
        }

        impl Render for Rows {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let scroll = self.scroll.0.borrow().base_handle.clone();
                gpui::uniform_list("rows", 500, move |range, _, _| {
                    let (first, last) = (range.start, range.end.saturating_sub(1));
                    range
                        .map(|index| {
                            let value: SharedString = row_text(index).into();
                            let text = StyledText::new(value.clone());
                            let layout = text.layout().clone();
                            div().h(px(20.0)).child(
                                selectable_line_in_document(
                                    9_000 + index as u64,
                                    9,
                                    index,
                                    value,
                                    layout,
                                    text,
                                    Vec::new(),
                                    |_, _, _| {},
                                    LineRow {
                                        first: index == first,
                                        last: index == last,
                                        source: Rc::new(|range, _| {
                                            Some(lines_text(range, |index| {
                                                (index < 500).then(|| row_text(index))
                                            }))
                                        }),
                                    },
                                )
                                .with_selection_scroll(scroll.clone()),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .track_scroll(self.scroll.clone())
                .w(px(240.0))
                .h(px(200.0))
            }
        }

        let scroll = gpui::UniformListScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll = scroll.clone();
            move |_, _| Rows { scroll }
        });
        cx.run_until_parked();
        // From inside "row 001" to inside "row 003".
        let start = point(px(12.0), px(30.0));
        let end = point(px(20.0), px(70.0));
        cx.simulate_mouse_move(start, None, Modifiers::default());
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let selected = cx.update(|_, cx| TextSelection::selected(cx)).unwrap();
        assert!(selected.contains("001\nrow 002\nro"), "{selected:?}");
        assert!(!selected.contains("000"), "{selected:?}");
        assert!(!selected.contains("004"), "{selected:?}");
    }

    #[test]
    fn a_selection_reads_the_same_either_way_it_was_dragged() {
        let forward = TextSelection {
            block: Some(1),
            text: "select this text".into(),
            anchor: 7,
            head: 11,
            dragging: false,
            pressed_link: None,
            lines: None,
        };
        let backward = TextSelection {
            anchor: 11,
            head: 7,
            ..forward.clone()
        };

        assert_eq!(forward.range(), 7..11);
        assert_eq!(backward.range(), 7..11);
        assert_eq!(&forward.text[forward.range()], "this");
    }

    #[test]
    fn selection_ranges_continue_across_text_blocks() {
        let selection = 2..16;

        assert_eq!(selection_range_in_block(&selection, &(0..10)), Some(2..10));
        assert_eq!(selection_range_in_block(&selection, &(12..23)), Some(0..4));
    }

    #[test]
    fn an_empty_selection_copies_nothing() {
        let empty = TextSelection {
            block: Some(1),
            text: "text".into(),
            anchor: 2,
            head: 2,
            dragging: false,
            pressed_link: None,
            lines: None,
        };
        assert_eq!(empty.range(), 2..2);
        assert!(empty.text.get(empty.range()).unwrap().is_empty());
    }

    #[test]
    fn a_double_click_selects_the_word_under_it() {
        assert_eq!(word_range("select this_text now", 9), 7..16);
        assert_eq!(word_range("select this_text now", 16), 16..17);
    }

    #[test]
    fn a_word_selection_uses_utf8_byte_offsets() {
        assert_eq!(word_range("say café now", 6), 4..9);
        assert_eq!(&"say café now"[word_range("say café now", 6)], "café");
    }

    #[test]
    fn a_word_selection_does_not_cross_a_newline() {
        assert_eq!(word_range("one\n  two", 3), 3..4);
        assert_eq!(word_range("one\n  two", 4), 4..6);
    }

    #[test]
    fn a_triple_click_selects_the_entire_line() {
        assert_eq!(
            range_for_click("first line\nsecond line\nthird", 14, 3),
            11..22
        );
        assert_eq!(
            range_for_click("first line\nsecond line\nthird", 14, 2),
            11..17
        );
        assert_eq!(line_range("first line\nsecond line\nthird", 0), 0..10);
        assert_eq!(line_range("first line\nsecond line\nthird", 28), 23..28);
    }

    #[test]
    fn dragging_selection_past_viewport_edges_advances_scroll_offset() {
        assert_eq!(
            selection_edge_scroll(px(-40.), px(200.), px(5.), px(10.), px(90.)),
            Some(px(-21.))
        );
        assert_eq!(
            selection_edge_scroll(px(-40.), px(200.), px(95.), px(10.), px(90.)),
            Some(px(-59.))
        );
        assert_eq!(
            selection_edge_scroll(px(-40.), px(200.), px(50.), px(10.), px(90.)),
            None
        );
        assert_eq!(
            selection_edge_scroll(px(-5.), px(200.), px(0.), px(10.), px(90.)),
            Some(px(0.))
        );
        assert_eq!(
            selection_edge_scroll(px(-195.), px(200.), px(100.), px(10.), px(90.)),
            Some(px(-200.))
        );
    }

    #[test]
    fn dragging_selection_into_the_viewport_edge_zone_scrolls_before_the_window_boundary() {
        assert_eq!(
            selection_edge_scroll(px(-40.), px(200.), px(15.), px(10.), px(90.)),
            Some(px(-21.))
        );
        assert_eq!(
            selection_edge_scroll(px(-40.), px(200.), px(85.), px(10.), px(90.)),
            Some(px(-59.))
        );
    }

    #[test]
    fn selection_auto_scroll_repeats_while_the_pointer_stays_past_an_edge() {
        let source = include_str!("selection.rs");
        let listener = source
            .split_once("fn listen(")
            .expect("selection listener")
            .1
            .split_once("/// The selected portion")
            .expect("end of selection listener")
            .0;

        assert!(listener.contains("schedule_selection_auto_scroll("));
        assert!(listener.contains("window.on_next_frame"));
    }
}
