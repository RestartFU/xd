use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div,
    fill, hsla, point, prelude::*, px, relative, rgb, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    editor::{next_word_boundary, previous_word_boundary},
    selection::TextSelection,
};

actions!(
    composer_input,
    [
        Backspace,
        Delete,
        DeleteWord,
        DeleteWordForward,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Submit,
    ]
);

actions!(terminal_input, [Up, Down, Interrupt, Escape, Tab]);

const CARET_WIDTH: Pixels = px(2.);

/// How far to scroll a single-line box so the caret stays inside it: never past
/// what the text overflows by, and only when the caret would otherwise leave.
fn caret_scroll(current: Pixels, caret: Pixels, text: Pixels, visible: Pixels) -> Pixels {
    let overflow = (text - visible + CARET_WIDTH).max(px(0.));
    let mut scroll = current.clamp(px(0.), overflow);
    if caret < scroll {
        scroll = caret;
    }
    let last_column = (visible - CARET_WIDTH).max(px(0.));
    if caret - scroll > last_column {
        scroll = caret - last_column;
    }
    scroll.clamp(px(0.), overflow)
}

#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Changed(String),
    Submit,
    Bytes(Vec<u8>),
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the single visible line is scrolled left, so text longer than the
    /// box follows the caret instead of running past the border.
    scroll: Pixels,
    is_selecting: bool,
    terminal: bool,
    concealed: bool,
}

impl EventEmitter<ComposerEvent> for ComposerInput {}

impl ComposerInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll: px(0.),
            is_selecting: false,
            terminal: false,
            concealed: false,
        }
    }

    pub fn terminal(cx: &mut Context<Self>) -> Self {
        let mut input = Self::new(cx, "");
        input.terminal = true;
        input
    }

    pub fn password(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        let mut input = Self::new(cx, placeholder);
        input.concealed = true;
        input
    }

    fn terminal_bytes(&self, bytes: impl Into<Vec<u8>>, cx: &mut Context<Self>) -> bool {
        if self.terminal {
            // Once the user resumes typing, a terminal output selection should
            // no longer intercept Ctrl+C as a copy on the next keypress.
            TextSelection::clear(cx);
            cx.emit(ComposerEvent::Bytes(bytes.into()));
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let content = text.into();
        if self.content == content {
            return;
        }
        self.content = content;
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Set the text and select all of it, so the first keypress replaces it.
    ///
    /// For a field that opens with a usable default rather than a blank: the
    /// default is there to be accepted, and typing over it is how it is
    /// declined. `set_text` alone leaves the caret past the end, where typing
    /// appends to a word the user never chose.
    pub fn set_text_selected(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.set_text(text, cx);
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(ComposerEvent::Changed(self.content.to_string()));
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\x1b[D".to_vec(), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\x1b[C".to_vec(), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            previous_word_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            next_word_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(
            previous_word_boundary(&self.content, self.cursor_offset()),
            cx,
        );
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_word_boundary(&self.content, self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\x1b[H".to_vec(), cx) {
            return;
        }
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\x1b[F".to_vec(), cx) {
            return;
        }
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(vec![0x7f], cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\x1b[3~".to_vec(), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word(&mut self, _: &DeleteWord, window: &mut Window, cx: &mut Context<Self>) {
        // A shell deletes the word behind the cursor on ctrl-w.
        if self.terminal_bytes(vec![0x17], cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let offset = previous_word_boundary(&self.content, self.cursor_offset());
            self.select_to(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_bytes(b"\x1bd".to_vec(), cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let offset = next_word_boundary(&self.content, self.cursor_offset());
            self.select_to(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_bytes(b"\r".to_vec(), cx) {
            return;
        }
        cx.emit(ComposerEvent::Submit);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal_bytes(b"\x1b[A".to_vec(), cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal_bytes(b"\x1b[B".to_vec(), cx);
    }
    fn interrupt(&mut self, _: &Interrupt, _: &mut Window, cx: &mut Context<Self>) {
        if self.terminal
            && let Some(text) = TextSelection::selected(cx)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        self.terminal_bytes(vec![3], cx);
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal_bytes(vec![27], cx);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal_bytes(vec![9], cx);
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if self.terminal_bytes(text.as_bytes().to_vec(), cx) {
                return;
            }
            self.replace_text_in_range(None, &text.replace(['\r', '\n'], " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            if !self.concealed {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    self.content[self.selected_range.clone()].to_string(),
                ));
            }
            return;
        }
        // The transcript cannot take focus, so a copy typed at an input with
        // nothing selected is meant for the text selected out there.
        if let Some(text) = TextSelection::selected(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            if !self.concealed {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    self.content[self.selected_range.clone()].to_string(),
                ));
            }
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.terminal {
            self.terminal_bytes(new_text.as_bytes().to_vec(), cx);
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + new_text + &self.content[range.end..]).into();
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| self.range_from_utf16(selection))
            .map(|selection| range.start + selection.start..range.start + selection.end)
            .unwrap_or_else(|| {
                let cursor = range.start + new_text.len();
                cursor..cursor
            });
        self.selection_reversed = false;
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(range.start) - self.scroll,
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(range.end) - self.scroll,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let local = self.last_bounds?.localize(&point)?;
        let layout = self.last_layout.as_ref()?;
        let index = layout.index_for_x(point.x - local.x)?;
        Some(self.offset_to_utf16(index))
    }
}

struct TextElement {
    input: Entity<ComposerInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = if input.concealed && !input.content.is_empty() {
            "*".repeat(input.content.len()).into()
        } else {
            input.content.clone()
        };
        let selected_range = input.selected_range.clone();
        let cursor_offset = input.cursor_offset();
        let style = window.text_style();
        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0.65, 1.))
        } else {
            (content, style.color)
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let previous_scroll = input.scroll;
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);
        let cursor_x = line.x_for_index(cursor_offset);
        let scroll = caret_scroll(previous_scroll, cursor_x, line.width, bounds.size.width);
        self.input.update(cx, |input, _| input.scroll = scroll);
        let left = bounds.left() - scroll;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(left + cursor_x, bounds.top()),
                        size(CARET_WIDTH, bounds.bottom() - bounds.top()),
                    ),
                    rgb(0x6b8cff),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(left + line.x_for_index(selected_range.start), bounds.top()),
                        point(left + line.x_for_index(selected_range.end), bounds.bottom()),
                    ),
                    rgba(0x6b8cff55),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().expect("input line was shaped");
        let terminal = self.input.read(cx).terminal;
        let scroll = self.input.read(cx).scroll;
        let origin = point(bounds.left() - scroll, bounds.top());
        // Text longer than the box is scrolled, so nothing may paint past it.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            if !terminal {
                line.paint(origin, window.line_height(), window, cx)
                    .expect("paint input line");
            }
            if !terminal
                && focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for ComposerInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .key_context("ComposerInput")
            .when(self.terminal, |input| {
                input
                    .key_context("TerminalInput")
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .w(px(1.0))
                    .h(px(1.0))
                    .overflow_hidden()
            })
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::interrupt))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::tab))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(px(22.))
            .text_size(px(14.))
            .text_color(rgb(0xe8eaf0))
            .child(
                div()
                    .h(px(30.))
                    .w_full()
                    .py_1()
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_stays_inside_a_box_narrower_than_its_text() {
        let visible = px(100.);
        // Text that fits never scrolls.
        assert_eq!(caret_scroll(px(0.), px(40.), px(80.), visible), px(0.));
        // Typing at the end pulls the tail into view.
        assert_eq!(caret_scroll(px(0.), px(300.), px(300.), visible), px(202.));
        // Moving back to the start returns to it.
        assert_eq!(caret_scroll(px(202.), px(0.), px(300.), visible), px(0.));
        // A caret already in view leaves the scroll alone.
        assert_eq!(
            caret_scroll(px(150.), px(200.), px(300.), visible),
            px(150.)
        );
        // Deleting text never leaves a gap past its end.
        assert_eq!(caret_scroll(px(202.), px(20.), px(20.), visible), px(0.));
    }
}
