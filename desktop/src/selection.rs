//! Mouse text selection for rendered text.
//!
//! GPUI paints text; it has no notion of selecting it. This wraps an already
//! laid-out text element so dragging across it highlights a range and Ctrl+C
//! copies that range. Each block of text is laid out on its own, so a selection
//! belongs to exactly one block.

use std::ops::Range;

use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, Global,
    GlobalElementId, Hitbox, HitboxBehavior, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, TextLayout, Window, fill, point,
    rgba,
};

const SELECTION: u32 = 0x6b8cff55;

/// The one selection in the window, keyed by the block that owns it.
#[derive(Clone, Default)]
pub struct TextSelection {
    block: Option<u64>,
    text: SharedString,
    anchor: usize,
    head: usize,
    dragging: bool,
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

    pub fn clear(cx: &mut App) {
        if cx
            .try_global::<Self>()
            .is_some_and(|selection| selection.block.is_some())
        {
            cx.set_global(Self::default());
        }
    }

    fn owns(cx: &App, block: u64) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|selection| selection.block == Some(block))
    }
}

/// Makes a laid-out text element selectable. `block` identifies the text, so a
/// drag that starts in one block replaces a selection held by another.
pub fn selectable(block: u64, layout: TextLayout, element: impl IntoElement) -> Selectable {
    Selectable {
        block,
        layout,
        element: Some(element.into_any_element()),
    }
}

pub struct Selectable {
    block: u64,
    layout: TextLayout,
    element: Option<AnyElement>,
}

impl Selectable {
    fn paint_selection(&self, window: &mut Window, cx: &App) {
        if !TextSelection::owns(cx, self.block) {
            return;
        }
        let range = cx
            .try_global::<TextSelection>()
            .map(TextSelection::range)
            .unwrap_or_default();
        if range.start >= range.end {
            return;
        }
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

    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &App) {
        let block = self.block;
        let layout = self.layout.clone();
        let pressed = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !pressed.is_hovered(window)
            {
                return;
            }
            let index = index_at(&layout, event.position);
            cx.set_global(TextSelection {
                block: Some(block),
                text: layout.text().into(),
                anchor: index,
                head: index,
                dragging: true,
            });
            window.refresh();
        });

        let layout = self.layout.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !event.dragging() {
                return;
            }
            if !cx
                .try_global::<TextSelection>()
                .is_some_and(|selection| selection.dragging && selection.block == Some(block))
            {
                return;
            }
            let index = index_at(&layout, event.position);
            let selection = cx.global_mut::<TextSelection>();
            if selection.head == index {
                return;
            }
            selection.head = index;
            window.refresh();
        });

        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && TextSelection::owns(cx, block) {
                cx.global_mut::<TextSelection>().dragging = false;
            }
        });

        // Only the block holding the selection needs to drop it, and it drops on
        // the way down so a press on other text can claim it on the way back up.
        if TextSelection::owns(cx, block) {
            window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    TextSelection::clear(cx);
                    window.refresh();
                }
            });
        }
    }
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
        element.paint(window, cx);
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        self.listen(hitbox, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_reads_the_same_either_way_it_was_dragged() {
        let forward = TextSelection {
            block: Some(1),
            text: "select this text".into(),
            anchor: 7,
            head: 11,
            dragging: false,
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
    fn an_empty_selection_copies_nothing() {
        let empty = TextSelection {
            block: Some(1),
            text: "text".into(),
            anchor: 2,
            head: 2,
            dragging: false,
        };
        assert_eq!(empty.range(), 2..2);
        assert!(empty.text.get(empty.range()).unwrap().is_empty());
    }
}
