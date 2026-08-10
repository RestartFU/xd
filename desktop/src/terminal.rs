use std::ops::Range;

const MAX_SCROLLBACK: usize = 5_000;
const MAX_GEOMETRY: usize = 1_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalStyle {
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    pub bold: bool,
    inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSpan {
    pub range: Range<usize>,
    pub style: TerminalStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalText {
    pub text: String,
    pub spans: Vec<TerminalSpan>,
    pub cursor: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    character: char,
    style: TerminalStyle,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            style: TerminalStyle::default(),
        }
    }
}

#[derive(Clone)]
pub struct TerminalScreen {
    columns: usize,
    rows: usize,
    grid: Vec<Vec<Cell>>,
    scrollback: Vec<Vec<Cell>>,
    row: usize,
    column: usize,
    saved: (usize, usize),
    parser: Parser,
    utf8: Vec<u8>,
    style: TerminalStyle,
}

#[derive(Clone, Default)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Csi(String),
    Osc {
        escaped: bool,
    },
}

impl TerminalScreen {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.clamp(1, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        Self {
            columns,
            rows,
            grid: vec![vec![Cell::default(); columns]; rows],
            scrollback: Vec::new(),
            row: 0,
            column: 0,
            saved: (0, 0),
            parser: Parser::Ground,
            utf8: Vec::new(),
            style: TerminalStyle::default(),
        }
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.clamp(1, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        for line in &mut self.grid {
            line.resize(columns, Cell::default());
        }
        let rows_above_viewport = self.row.saturating_sub(rows - 1);
        for _ in 0..rows_above_viewport {
            self.scrollback.push(self.grid.remove(0));
        }
        self.row = self.row.saturating_sub(rows_above_viewport);
        self.saved.0 = self.saved.0.saturating_sub(rows_above_viewport);
        self.grid.truncate(rows);
        while self.grid.len() < rows {
            self.grid.push(vec![Cell::default(); columns]);
        }
        self.columns = columns;
        self.rows = rows;
        self.row = self.row.min(rows - 1);
        self.column = self.column.min(columns - 1);
        self.trim_scrollback();
    }

    pub fn geometry(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match &mut self.parser {
                Parser::Ground => match byte {
                    0x1b => self.parser = Parser::Escape,
                    b'\r' => self.column = 0,
                    b'\n' => self.line_feed(),
                    0x08 => self.column = self.column.saturating_sub(1),
                    b'\t' => self.column = ((self.column / 8 + 1) * 8).min(self.columns - 1),
                    0x20..=0x7e => self.put(byte as char),
                    0x80..=0xff => self.feed_utf8(byte),
                    _ => {}
                },
                Parser::Escape => match byte {
                    b'[' => self.parser = Parser::Csi(String::new()),
                    b']' => self.parser = Parser::Osc { escaped: false },
                    b'7' => {
                        self.saved = (self.row, self.column);
                        self.parser = Parser::Ground;
                    }
                    b'8' => {
                        (self.row, self.column) = self.saved;
                        self.parser = Parser::Ground;
                    }
                    b'c' => {
                        self.reset();
                        self.parser = Parser::Ground;
                    }
                    _ => self.parser = Parser::Ground,
                },
                Parser::Csi(sequence) => {
                    if (0x40..=0x7e).contains(&byte) {
                        let sequence = std::mem::take(sequence);
                        self.csi(&sequence, byte as char);
                        self.parser = Parser::Ground;
                    } else if sequence.len() < 128 {
                        sequence.push(byte as char);
                    } else {
                        self.parser = Parser::Ground;
                    }
                }
                Parser::Osc { escaped } => {
                    if byte == 0x07 || (*escaped && byte == b'\\') {
                        self.parser = Parser::Ground;
                    } else {
                        *escaped = byte == 0x1b;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.rendered().text
    }

    #[cfg(test)]
    pub fn rendered(&self) -> TerminalText {
        self.rendered_inner(false)
    }

    pub fn rendered_with_cursor(&self) -> TerminalText {
        self.rendered_inner(true)
    }

    fn rendered_inner(&self, include_cursor: bool) -> TerminalText {
        let mut lines = self
            .scrollback
            .iter()
            .chain(self.grid.iter())
            .collect::<Vec<_>>();
        let cursor_line = self.scrollback.len() + self.row;
        while lines
            .last()
            .is_some_and(|line| line.iter().all(|cell| cell.character == ' '))
            && lines.len() > 1
            && (!include_cursor || lines.len() > cursor_line + 1)
        {
            lines.pop();
        }
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut cursor = None;
        for (line_index, line) in lines.into_iter().enumerate() {
            if line_index > 0 {
                text.push('\n');
            }
            let content_end = line
                .iter()
                .rposition(|cell| cell.character != ' ')
                .map_or(0, |index| index + 1);
            let cursor_column = self.column.min(self.columns.saturating_sub(1));
            let end = if include_cursor && line_index == cursor_line {
                content_end.max(cursor_column + 1)
            } else {
                content_end
            };
            let mut run: Option<(usize, TerminalStyle)> = None;
            for (column, cell) in line[..end].iter().enumerate() {
                let style = cell.style.resolved();
                if run.is_some_and(|(_, current)| current != style) {
                    let (start, current) = run.take().expect("terminal style run exists");
                    if current != TerminalStyle::default() {
                        spans.push(TerminalSpan {
                            range: start..text.len(),
                            style: current,
                        });
                    }
                }
                if run.is_none() {
                    run = Some((text.len(), style));
                }
                let cell_start = text.len();
                text.push(cell.character);
                if include_cursor && line_index == cursor_line && column == cursor_column {
                    cursor = Some(cell_start..text.len());
                }
            }
            if let Some((start, style)) = run
                && style != TerminalStyle::default()
            {
                spans.push(TerminalSpan {
                    range: start..text.len(),
                    style,
                });
            }
        }
        TerminalText {
            text,
            spans,
            cursor,
        }
    }

    fn put(&mut self, character: char) {
        if self.column >= self.columns {
            self.column = 0;
            self.line_feed();
        }
        self.grid[self.row][self.column] = Cell {
            character,
            style: self.style,
        };
        self.column += 1;
    }

    fn feed_utf8(&mut self, byte: u8) {
        self.utf8.push(byte);
        match std::str::from_utf8(&self.utf8) {
            Ok(text) => {
                let characters = text.chars().collect::<Vec<_>>();
                self.utf8.clear();
                for character in characters {
                    self.put(character);
                }
            }
            Err(error) if error.error_len().is_some() || self.utf8.len() >= 4 => {
                self.utf8.clear();
                self.put('�');
            }
            Err(_) => {}
        }
    }

    fn line_feed(&mut self) {
        if self.row + 1 < self.rows {
            self.row += 1;
        } else {
            self.scrollback.push(self.grid.remove(0));
            self.grid.push(vec![Cell::default(); self.columns]);
            self.trim_scrollback();
        }
    }

    fn trim_scrollback(&mut self) {
        if self.scrollback.len() > MAX_SCROLLBACK {
            self.scrollback
                .drain(..self.scrollback.len() - MAX_SCROLLBACK);
        }
    }

    fn reset(&mut self) {
        self.grid = vec![vec![Cell::default(); self.columns]; self.rows];
        self.scrollback.clear();
        self.row = 0;
        self.column = 0;
        self.saved = (0, 0);
        self.style = TerminalStyle::default();
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            1 => {
                for row in 0..self.row {
                    self.grid[row].fill(Cell::default());
                }
                for column in 0..=self.column.min(self.columns - 1) {
                    self.grid[self.row][column] = Cell::default();
                }
            }
            2 => {
                for row in &mut self.grid {
                    row.fill(Cell::default());
                }
            }
            // xterm's erase-saved-lines extension is emitted by `clear` after
            // erasing the visible display. The renderer includes saved lines,
            // so retaining them makes a successful clear appear to do nothing.
            3 => self.scrollback.clear(),
            _ => {
                for column in self.column..self.columns {
                    self.grid[self.row][column] = Cell::default();
                }
                for row in self.row + 1..self.rows {
                    self.grid[row].fill(Cell::default());
                }
            }
        }
    }

    fn csi(&mut self, sequence: &str, command: char) {
        let sequence = sequence.trim_start_matches(['?', '>']);
        let values = sequence
            .split(';')
            .map(|value| value.parse::<usize>().unwrap_or(0))
            .collect::<Vec<_>>();
        let first = values.first().copied().unwrap_or(0);
        let amount = first.max(1);
        match command {
            'A' => self.row = self.row.saturating_sub(amount),
            'B' => self.row = (self.row + amount).min(self.rows - 1),
            'C' => self.column = (self.column + amount).min(self.columns - 1),
            'D' => self.column = self.column.saturating_sub(amount),
            'G' => self.column = amount.saturating_sub(1).min(self.columns - 1),
            'd' => self.row = amount.saturating_sub(1).min(self.rows - 1),
            'H' | 'f' => {
                self.row = values
                    .first()
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .saturating_sub(1)
                    .min(self.rows - 1);
                self.column = values
                    .get(1)
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .saturating_sub(1)
                    .min(self.columns - 1);
            }
            'J' => self.erase_display(first),
            'K' if first == 2 => self.grid[self.row].fill(Cell::default()),
            'K' if first == 1 => {
                for column in 0..=self.column.min(self.columns - 1) {
                    self.grid[self.row][column] = Cell::default();
                }
            }
            'K' => {
                for column in self.column..self.columns {
                    self.grid[self.row][column] = Cell::default();
                }
            }
            'P' => {
                let column = self.column.min(self.columns);
                let amount = amount.min(self.columns - column);
                self.grid[self.row].copy_within(column + amount..self.columns, column);
                self.grid[self.row][self.columns - amount..].fill(Cell::default());
            }
            'm' => self.sgr(&values),
            's' => self.saved = (self.row, self.column),
            'u' => (self.row, self.column) = self.saved,
            _ => {}
        }
    }

    fn sgr(&mut self, values: &[usize]) {
        let mut index = 0;
        while index < values.len() {
            let value = values[index];
            match value {
                0 => self.style = TerminalStyle::default(),
                1 => self.style.bold = true,
                22 => self.style.bold = false,
                7 => self.style.inverse = true,
                27 => self.style.inverse = false,
                30..=37 => self.style.foreground = Some(ansi_color(value - 30)),
                39 => self.style.foreground = None,
                40..=47 => self.style.background = Some(ansi_color(value - 40)),
                49 => self.style.background = None,
                90..=97 => self.style.foreground = Some(ansi_color(value - 90 + 8)),
                100..=107 => self.style.background = Some(ansi_color(value - 100 + 8)),
                38 | 48 => {
                    let foreground = value == 38;
                    let color = match values.get(index + 1).copied() {
                        Some(5) => {
                            index += 2;
                            values.get(index).copied().map(xterm_color)
                        }
                        Some(2) if index + 4 < values.len() => {
                            let red = values[index + 2].min(255) as u32;
                            let green = values[index + 3].min(255) as u32;
                            let blue = values[index + 4].min(255) as u32;
                            index += 4;
                            Some((red << 16) | (green << 8) | blue)
                        }
                        _ => None,
                    };
                    if foreground {
                        self.style.foreground = color;
                    } else {
                        self.style.background = color;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

impl TerminalStyle {
    fn resolved(mut self) -> Self {
        if self.inverse {
            std::mem::swap(&mut self.foreground, &mut self.background);
            self.inverse = false;
        }
        self
    }
}

fn ansi_color(index: usize) -> u32 {
    const COLORS: [u32; 16] = [
        0x1f2329, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xd7dae0, 0x5c6370,
        0xff7a85, 0xb3e180, 0xffd68a, 0x7dbaff, 0xd99bff, 0x75d5e0, 0xffffff,
    ];
    COLORS[index.min(COLORS.len() - 1)]
}

fn xterm_color(index: usize) -> u32 {
    match index.min(255) {
        0..=15 => ansi_color(index),
        16..=231 => {
            let value = index - 16;
            let component = |part: usize| -> u32 {
                const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
                LEVELS[part.min(5)]
            };
            let red = component(value / 36);
            let green = component((value / 6) % 6);
            let blue = component(value % 6);
            (red << 16) | (green << 8) | blue
        }
        232..=255 => {
            let gray = 8 + (index - 232) as u32 * 10;
            (gray << 16) | (gray << 8) | gray
        }
        _ => unreachable!(),
    }
}

impl Default for TerminalScreen {
    fn default() -> Self {
        Self::new(120, 32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_cursor_control_without_duplicate_prompt_lines() {
        let mut screen = TerminalScreen::new(12, 3);
        screen.feed(b"hello\rbye\r\nnext\x1b[1A\x1b[6G!");
        assert_eq!(screen.text(), "byelo!\nnext");
    }

    #[test]
    fn readline_history_deletes_the_old_command_suffix() {
        let mut screen = TerminalScreen::new(24, 3);
        screen.feed(b"$ killport 19132");

        // Bash/readline emits this when Down replaces the longer history entry
        // with `make server`: return to the prompt, delete the three extra
        // cells, then overwrite the retained prefix.
        screen.feed(b"\r\x1b[C\x1b[C\x1b[3Pmake server");

        assert_eq!(screen.text(), "$ make server");
    }

    #[test]
    fn resizing_shorter_keeps_a_visible_prompt_on_its_original_row() {
        let mut screen = TerminalScreen::new(20, 4);
        screen.feed(b"new terminal");

        for rows in [3, 4, 2, 4] {
            screen.resize(20, rows);
            screen.feed(b"\r\x1b[2Knew terminal");
        }

        assert_eq!(screen.text(), "new terminal");
        assert!(screen.scrollback.is_empty());
    }

    #[test]
    fn clear_sequence_erases_the_display_and_scrollback() {
        let mut screen = TerminalScreen::new(12, 2);
        screen.feed(b"old one\nold two\nold three");
        assert!(!screen.scrollback.is_empty());

        // xterm-256color's `clear` capability: home, erase the display, then
        // erase saved lines. The following prompt may arrive in another frame.
        screen.feed(b"\x1b[H\x1b[2J\x1b[3J");

        assert_eq!(screen.text(), "");
        assert!(screen.scrollback.is_empty());
        assert_eq!((screen.row, screen.column), (0, 0));

        screen.feed(b"prompt> ");
        assert_eq!(screen.text(), "prompt>");
    }

    #[test]
    fn erase_display_modes_preserve_the_cursor_position() {
        let mut screen = TerminalScreen::new(12, 3);
        screen.feed(b"first\nsecond\nthird");
        let cursor = (screen.row, screen.column);

        screen.feed(b"\x1b[2J");

        assert_eq!((screen.row, screen.column), cursor);
        assert!(
            screen
                .grid
                .iter()
                .flatten()
                .all(|cell| cell.character == ' ')
        );
        assert_eq!(screen.text(), "first");
    }

    #[test]
    fn bounds_scrollback_and_preserves_split_utf8() {
        let mut screen = TerminalScreen::new(8, 2);
        screen.feed(&[0xe2, 0x82]);
        screen.feed(&[0xac, b'\n']);
        for _ in 0..(MAX_SCROLLBACK + 20) {
            screen.feed(b"line\n");
        }
        assert!(screen.scrollback.len() <= MAX_SCROLLBACK);
        assert!(screen.text().contains("line"));
    }

    #[test]
    fn preserves_standard_extended_and_truecolor_sgr_styles() {
        let mut screen = TerminalScreen::new(20, 2);
        screen.feed(b"plain \x1b[31;1mred\x1b[0m \x1b[38;5;82mgreen\x1b[0m \x1b[48;2;1;2;3mX");
        let rendered = screen.rendered();
        assert_eq!(rendered.text, "plain red green X");
        assert!(rendered.spans.iter().any(|span| {
            &rendered.text[span.range.clone()] == "red"
                && span.style.foreground == Some(ansi_color(1))
                && span.style.bold
        }));
        assert!(rendered.spans.iter().any(|span| {
            &rendered.text[span.range.clone()] == "green"
                && span.style.foreground == Some(xterm_color(82))
        }));
        assert!(rendered.spans.iter().any(|span| {
            &rendered.text[span.range.clone()] == "X" && span.style.background == Some(0x010203)
        }));
    }

    #[test]
    fn exposes_the_cursor_cell_without_changing_plain_rendering() {
        let mut screen = TerminalScreen::new(8, 3);
        screen.feed(b"prompt> ");
        assert_eq!(screen.rendered().text, "prompt>");

        let rendered = screen.rendered_with_cursor();
        let cursor = rendered.cursor.expect("cursor cell");
        assert_eq!(&rendered.text[cursor], " ");
    }
}
