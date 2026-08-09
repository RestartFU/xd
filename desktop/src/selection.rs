//! Mouse text selection for rendered text.
//!
//! GPUI paints text; it has no notion of selecting it. This wraps an already
//! laid-out text element so dragging across it highlights a range, double-click
//! selects a word, and Ctrl+C copies that range. Markdown lays each block out
//! separately, so every selectable carries its range in one shared document.

use std::ops::Range;

use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, Global,
    GlobalElementId, Hitbox, HitboxBehavior, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, TextLayout, Window, fill, point,
    rgba,
};

const SELECTION: u32 = 0x6b8cff55;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionSource {
    ChatMessage {
        chat_id: String,
        chat_title: String,
        message_id: Option<i64>,
        role: String,
    },
    WorkspaceFile {
        chat_id: String,
        path: String,
    },
}

#[derive(Clone)]
pub struct SelectedTextContext {
    pub text: String,
    pub source: SelectionSource,
    pub position: Point<Pixels>,
    pub menu_open: bool,
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
    source: Option<SelectionSource>,
    action_position: Option<Point<Pixels>>,
    action_menu_open: bool,
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
        selection
            .text
            .get(selection.range())
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    }

    pub fn context(cx: &App) -> Option<SelectedTextContext> {
        let selection = cx.try_global::<Self>()?;
        let text = selection
            .text
            .get(selection.range())
            .filter(|text| !text.is_empty())?
            .to_owned();
        Some(SelectedTextContext {
            text,
            source: selection.source.clone()?,
            position: selection.action_position?,
            menu_open: selection.action_menu_open,
        })
    }

    pub fn toggle_action_menu(cx: &mut App) {
        if cx.try_global::<Self>().is_some() {
            let selection = cx.global_mut::<Self>();
            selection.action_menu_open = !selection.action_menu_open;
        }
    }

    pub fn close_action_menu(cx: &mut App) -> bool {
        if cx.try_global::<Self>().is_none() {
            return false;
        }
        std::mem::take(&mut cx.global_mut::<Self>().action_menu_open)
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
    source: Option<SelectionSource>,
    layout: TextLayout,
    element: impl IntoElement,
) -> Selectable {
    Selectable {
        block,
        document,
        document_text: Some(text),
        document_range: Some(range),
        source,
        layout,
        element: Some(element.into_any_element()),
        links: Vec::new(),
        link_listener: None,
    }
}

/// Makes text selectable while preserving clickable character ranges. Keeping
/// selection and link activation on one hit target avoids competing cursors and
/// lets the pressed link survive redraws between mouse-down and mouse-up.
pub fn selectable_links(
    block: u64,
    layout: TextLayout,
    element: impl IntoElement,
    links: Vec<Range<usize>>,
    listener: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Selectable {
    Selectable {
        block,
        document: block,
        document_text: None,
        document_range: None,
        source: None,
        layout,
        element: Some(element.into_any_element()),
        links,
        link_listener: Some(Box::new(listener)),
    }
}

pub fn selectable_links_in_document(
    block: u64,
    document: u64,
    text: SharedString,
    range: Range<usize>,
    source: Option<SelectionSource>,
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
        source,
        layout,
        element: Some(element.into_any_element()),
        links,
        link_listener: Some(Box::new(listener)),
    }
}

type LinkListener = Box<dyn Fn(usize, &mut Window, &mut App)>;

pub struct Selectable {
    block: u64,
    document: u64,
    document_text: Option<SharedString>,
    document_range: Option<Range<usize>>,
    source: Option<SelectionSource>,
    layout: TextLayout,
    element: Option<AnyElement>,
    links: Vec<Range<usize>>,
    link_listener: Option<LinkListener>,
}

impl Selectable {
    fn paint_selection(&self, window: &mut Window, cx: &App) {
        if !TextSelection::owns(cx, self.document) {
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
        if start.y == end.y {
            paint_row(window, start, point(end.x, start.y + height));
            return;
        }
        paint_row(window, start, point(bounds.right(), start.y + height));
        if end.y > start.y + height {
            paint_row(
                window,
                point(bounds.left(), start.y + height),
                point(bounds.right(), end.y),
            );
        }
        paint_row(
            window,
            point(bounds.left(), end.y),
            point(end.x, end.y + height),
        );
    }

    fn listen(
        &self,
        hitbox: &Hitbox,
        link_listener: Option<LinkListener>,
        window: &mut Window,
        _cx: &App,
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
        let source = self.source.clone();
        let links = self.links.clone();
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
            let range = if event.click_count > 1 {
                word_range(&text, index)
            } else {
                index..index
            };
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
                source: source.clone(),
                action_position: Some(event.position),
                action_menu_open: false,
            });
            window.refresh();
        });

        let document_range = self
            .document_range
            .clone()
            .unwrap_or_else(|| 0..self.layout.len());
        let layout = self.layout.clone();
        let moved = hitbox.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || !event.dragging()
                || event.position.y < moved.top()
                || event.position.y > moved.bottom()
            {
                return;
            }
            if !cx
                .try_global::<TextSelection>()
                .is_some_and(|selection| selection.dragging && selection.block == Some(document))
            {
                return;
            }
            let index = document_range.start + index_at(&layout, event.position);
            let selection = cx.global_mut::<TextSelection>();
            if selection.head == index {
                return;
            }
            selection.head = index;
            selection.action_position = Some(event.position);
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
                selection.action_position = Some(event.position);
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
    }
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

fn paint_row(window: &mut Window, start: Point<Pixels>, end: Point<Pixels>) {
    if end.x <= start.x || end.y <= start.y {
        return;
    }
    window.paint_quad(fill(Bounds::from_corners(start, end), rgba(SELECTION)));
}

/// The byte offset a point lands on. Dragging past the text reports the nearest
/// offset, which is what carries a selection to the end of a line.
fn index_at(layout: &TextLayout, position: Point<Pixels>) -> usize {
    match layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    }
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
        Context, ListAlignment, ListState, Modifiers, Render, Styled, StyledText, TestAppContext,
        list, px,
    };

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
                    let text = StyledText::new("open link");
                    let layout = text.layout().clone();
                    selectable_links(1, layout, text, vec![0..9], |_, _, cx| {
                        cx.open_url("https://example.com")
                    })
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
                ..Default::default()
            });
        });
        let link = point(px(10.0), px(10.0));
        cx.simulate_mouse_move(link, None, Modifiers::default());
        cx.simulate_mouse_down(link, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(link, MouseButton::Left, Modifiers::default());

        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
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
            ..Default::default()
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
            ..Default::default()
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
}
