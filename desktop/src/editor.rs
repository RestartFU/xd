use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, actions, div, fill, point,
    prelude::*, px, relative, rgb, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    file_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Newline,
        Submit,
        Tab,
    ]
);

#[derive(Clone, Debug)]
pub enum EditorEvent {
    Changed(String),
    Submit,
}

pub struct FileEditor {
    focus_handle: FocusHandle,
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<PaintedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    composer: bool,
    placeholder: SharedString,
}

#[derive(Clone)]
struct PaintedLine {
    range: Range<usize>,
    layout: ShapedLine,
    bounds: Bounds<Pixels>,
}

impl EventEmitter<EditorEvent> for FileEditor {}

impl FileEditor {
    const LINE_HEIGHT: f32 = 20.0;

    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            composer: false,
            placeholder: "".into(),
        }
    }

    pub fn composer(cx: &mut Context<Self>) -> Self {
        Self {
            composer: true,
            placeholder: "Message xd…".into(),
            ..Self::new(cx)
        }
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Changed(self.content.to_string()));
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let start = self.content[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let end = self.content[cursor..]
            .find('\n')
            .map_or(self.content.len(), |index| cursor + index);
        self.move_to(end, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Submit);
    }

    fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "    ", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let offset = self.index_for_point(event.position);
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
            self.select_to(self.index_for_point(event.position), cx);
        }
    }

    fn move_vertical(&mut self, direction: isize, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let ranges = line_ranges(&self.content);
        let current = ranges
            .iter()
            .position(|range| cursor >= range.start && cursor <= range.end)
            .unwrap_or(0);
        let target = current
            .saturating_add_signed(direction)
            .min(ranges.len() - 1);
        let column = self.content[ranges[current].start..cursor].chars().count();
        let offset = self.content[ranges[target].clone()]
            .char_indices()
            .nth(column)
            .map_or(ranges[target].end, |(index, _)| {
                ranges[target].start + index
            });
        self.move_to(offset, cx);
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
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

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
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

    fn index_for_point(&self, point: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        if point.y <= bounds.top() {
            return 0;
        }
        let line = self
            .last_lines
            .iter()
            .find(|line| point.y < line.bounds.bottom())
            .or_else(|| self.last_lines.last());
        line.map_or(0, |line| {
            line.range.start
                + line
                    .layout
                    .closest_index_for_x(point.x - line.bounds.left())
                    .min(line.range.end - line.range.start)
        })
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        self.content
            .chars()
            .scan((0, 0), |state, character| {
                let current = *state;
                state.0 += character.len_utf16();
                state.1 += character.len_utf8();
                Some(current)
            })
            .find_map(|(utf16, utf8)| (utf16 >= offset).then_some(utf8))
            .unwrap_or(self.content.len())
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].encode_utf16().count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for FileEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
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
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + text + &self.content[range.end..]).into();
        let cursor = range.start + text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[..range.start].to_owned() + text + &self.content[range.end..]).into();
        self.marked_range = (!text.is_empty()).then_some(range.start..range.start + text.len());
        self.selected_range = selected
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|selection| range.start + selection.start..range.start + selection.end)
            .unwrap_or_else(|| {
                let cursor = range.start + text.len();
                cursor..cursor
            });
        self.selection_reversed = false;
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let offset = self.range_from_utf16(&range).start;
        self.last_lines
            .iter()
            .find(|line| offset >= line.range.start && offset <= line.range.end)
            .map(|line| {
                let x = line.layout.x_for_index(offset - line.range.start);
                Bounds::new(
                    point(line.bounds.left() + x, line.bounds.top()),
                    size(px(2.), line.bounds.size.height),
                )
            })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_point(point)))
    }
}

struct EditorElement {
    input: Entity<FileEditor>,
}

struct PrepaintState {
    lines: Vec<PaintedLine>,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
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
    ) -> (LayoutId, ()) {
        let count = line_ranges(&self.input.read(cx).content).len().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(count as f32 * FileEditor::LINE_HEIGHT).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let ranges = line_ranges(&input.content);
        let font_size = px(13.);
        let style = window.text_style();
        let mut lines = Vec::with_capacity(ranges.len());
        let mut selection = Vec::new();
        let cursor_offset = input.cursor_offset();
        let mut cursor = None;
        for (index, range) in ranges.into_iter().enumerate() {
            let placeholder =
                input.content.is_empty() && index == 0 && !input.placeholder.is_empty();
            let text: SharedString = if placeholder {
                input.placeholder.clone()
            } else {
                input.content[range.clone()].to_owned().into()
            };
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: rgb(if placeholder { 0xa8a8ad } else { 0xdde1ea }).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let layout = window
                .text_system()
                .shape_line(text, font_size, &[run], None);
            let line_bounds = Bounds::new(
                point(
                    bounds.left(),
                    bounds.top() + px(index as f32 * FileEditor::LINE_HEIGHT),
                ),
                size(bounds.size.width, px(FileEditor::LINE_HEIGHT)),
            );
            let selected_start = input.selected_range.start.max(range.start);
            let selected_end = input.selected_range.end.min(range.end);
            if selected_start < selected_end
                || (input.selected_range.start <= range.end && input.selected_range.end > range.end)
            {
                let x1 = layout.x_for_index(selected_start.saturating_sub(range.start));
                let x2 = if selected_end > selected_start {
                    layout.x_for_index(selected_end - range.start)
                } else {
                    layout.width
                };
                selection.push(fill(
                    Bounds::from_corners(
                        point(line_bounds.left() + x1, line_bounds.top()),
                        point(
                            line_bounds.left() + x2.max(x1 + px(4.)),
                            line_bounds.bottom(),
                        ),
                    ),
                    rgba(0x6b8cff55),
                ));
            }
            if input.selected_range.is_empty()
                && cursor_offset >= range.start
                && cursor_offset <= range.end
            {
                let x = layout.x_for_index(cursor_offset - range.start);
                cursor = Some(fill(
                    Bounds::new(
                        point(line_bounds.left() + x, line_bounds.top()),
                        size(px(2.), line_bounds.size.height),
                    ),
                    rgb(0x6b8cff),
                ));
            }
            lines.push(PaintedLine {
                range,
                layout,
                bounds: line_bounds,
            });
        }
        PrepaintState {
            lines,
            selection,
            cursor,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in state.selection.drain(..) {
            window.paint_quad(selection);
        }
        for line in &state.lines {
            line.layout
                .paint(line.bounds.origin, px(FileEditor::LINE_HEIGHT), window, cx)
                .expect("paint file editor line");
        }
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_lines = state.lines.clone();
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .min_h_full()
            .key_context("FileEditor")
            .when(self.composer, |editor| editor.key_context("MessageEditor"))
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::tab))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .font_family("monospace")
            .child(EditorElement { input: cx.entity() })
    }
}

impl Focusable for FileEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn line_ranges(content: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, _) in content.match_indices('\n') {
        ranges.push(start..index);
        start = index + 1;
    }
    ranges.push(start..content.len());
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ranges_preserve_empty_and_trailing_lines() {
        assert_eq!(line_ranges(""), vec![0..0]);
        assert_eq!(line_ranges("a\n\nb\n"), vec![0..1, 2..2, 3..4, 5..5]);
    }
}
