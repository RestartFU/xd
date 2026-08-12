use std::ops::Range;

use bincode::{Decode, Encode};

const MAX_SCROLLBACK: usize = 5_000;
const MAX_GEOMETRY: usize = 1_000;
const MAX_OSC_BYTES: usize = 4_096;
const MAX_HYPERLINKS: usize = 4_096;
const MAX_HYPERLINK_BYTES: usize = 1024 * 1024;
const MAX_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;
const CHECKPOINT_HEADER: &[u8] = b"XDTS\x01";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Encode, Decode)]
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
pub struct TerminalLink {
    pub range: Range<usize>,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalText {
    pub text: String,
    pub spans: Vec<TerminalSpan>,
    pub links: Vec<TerminalLink>,
    pub cursor: Option<Range<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
struct Cell {
    character: char,
    style: TerminalStyle,
    width: u8,
    combining: [char; 2],
    combining_len: u8,
    hyperlink: Option<u16>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            style: TerminalStyle::default(),
            width: 1,
            combining: ['\0'; 2],
            combining_len: 0,
            hyperlink: None,
        }
    }
}

#[derive(Clone, Encode, Decode)]
struct SavedScreen {
    grid: Vec<Vec<Cell>>,
    scrollback: Vec<Vec<Cell>>,
    row: usize,
    column: usize,
    saved: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    origin_mode: bool,
    auto_wrap: bool,
    insert_mode: bool,
}

#[derive(Clone, Encode, Decode)]
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
    hyperlinks: Vec<String>,
    active_hyperlink: Option<u16>,
    bracketed_paste: bool,
    cursor_visible: bool,
    synchronized_output: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    origin_mode: bool,
    auto_wrap: bool,
    insert_mode: bool,
    primary_screen: Option<Box<SavedScreen>>,
}

#[derive(Clone, Default, Encode, Decode)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Csi(String),
    Osc {
        sequence: Vec<u8>,
        escaped: bool,
        overflowed: bool,
    },
    String {
        escaped: bool,
    },
    Charset,
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
            hyperlinks: Vec::new(),
            active_hyperlink: None,
            bracketed_paste: false,
            cursor_visible: true,
            synchronized_output: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            origin_mode: false,
            auto_wrap: true,
            insert_mode: false,
            primary_screen: None,
        }
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.clamp(1, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        let old_rows = self.rows;
        let region_was_full = self.scroll_top == 0 && self.scroll_bottom + 1 == old_rows;
        for line in &mut self.grid {
            line.resize(columns, Cell::default());
            sanitize_line(line);
        }
        let rows_above_viewport = self.row.saturating_sub(rows - 1);
        for _ in 0..rows_above_viewport {
            let removed = self.grid.remove(0);
            if self.primary_screen.is_none() {
                self.scrollback.push(removed);
            }
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
        self.saved.0 = self.saved.0.min(rows - 1);
        self.saved.1 = self.saved.1.min(columns - 1);
        if region_was_full {
            self.scroll_top = 0;
            self.scroll_bottom = rows - 1;
        } else {
            self.scroll_top = self.scroll_top.min(rows - 1);
            self.scroll_bottom = self.scroll_bottom.min(rows - 1);
            if self.scroll_top >= self.scroll_bottom {
                self.scroll_top = 0;
                self.scroll_bottom = rows - 1;
            }
        }
        if let Some(primary) = &mut self.primary_screen {
            resize_saved_screen(primary, columns, rows);
        }
        self.trim_scrollback();
    }

    pub fn geometry(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// Encodes an exact, versioned terminal checkpoint no larger than
    /// `max_bytes`. When necessary, only the oldest scrollback rows are
    /// discarded; the viewport and in-flight parser state are never altered.
    pub fn checkpoint_bytes_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let max_bytes = max_bytes.min(MAX_CHECKPOINT_BYTES);
        if max_bytes < CHECKPOINT_HEADER.len() || !self.is_valid() {
            return None;
        }

        let encode = |screen: &Self| {
            let payload = bincode::encode_to_vec(
                screen,
                bincode::config::standard().with_limit::<MAX_CHECKPOINT_BYTES>(),
            )
            .ok()?;
            let encoded_len = CHECKPOINT_HEADER.len().checked_add(payload.len())?;
            let mut bytes = Vec::with_capacity(encoded_len);
            bytes.extend_from_slice(CHECKPOINT_HEADER);
            bytes.extend_from_slice(&payload);
            Some(bytes)
        };

        let scrollback_rows = self.scrollback_rows();
        let full = encode(self);
        if full.as_ref().is_some_and(|bytes| bytes.len() <= max_bytes) {
            return full;
        }
        if scrollback_rows == 0 {
            return None;
        }

        // Estimate a conservative retained suffix from the full encoded size,
        // then shrink monotonically if unusually dense later rows still do not
        // fit. This avoids repeatedly cloning the whole terminal during a
        // binary search when a busy CLI first crosses the replay watermark.
        let mut retained = full.as_ref().map_or(scrollback_rows.min(1_000), |bytes| {
            scrollback_rows
                .saturating_mul(max_bytes)
                .checked_div(bytes.len().max(1))
                .unwrap_or(0)
                .saturating_mul(9)
                / 10
        });
        retained = retained.min(scrollback_rows.saturating_sub(1));
        let mut candidate = self.clone();
        candidate.remove_oldest_scrollback(scrollback_rows - retained);

        loop {
            if let Some(bytes) = encode(&candidate) {
                if bytes.len() <= max_bytes {
                    return Some(bytes);
                }
                if retained == 0 {
                    return None;
                }
                let next = retained
                    .saturating_mul(max_bytes)
                    .checked_div(bytes.len().max(1))
                    .unwrap_or(0)
                    .saturating_mul(9)
                    / 10;
                let next = next.min(retained - 1);
                candidate.remove_oldest_scrollback(retained - next);
                retained = next;
            } else {
                if retained == 0 {
                    return None;
                }
                let next = retained / 2;
                candidate.remove_oldest_scrollback(retained - next);
                retained = next;
            }
        }
    }

    /// Restores a checkpoint emitted by [`Self::checkpoint_bytes_bounded`].
    /// Unknown versions, trailing data, excessive allocations, and malformed
    /// terminal state are rejected instead of being repaired implicitly.
    pub fn from_checkpoint_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_CHECKPOINT_BYTES || !bytes.starts_with(CHECKPOINT_HEADER) {
            return None;
        }
        let payload = &bytes[CHECKPOINT_HEADER.len()..];
        let (screen, consumed): (Self, usize) = bincode::decode_from_slice(
            payload,
            bincode::config::standard().with_limit::<MAX_CHECKPOINT_BYTES>(),
        )
        .ok()?;
        (consumed == payload.len() && screen.is_valid()).then_some(screen)
    }

    /// Produces a self-contained ANSI reconstruction for clients that do not
    /// understand binary checkpoints. This is a clean terminal snapshot, not
    /// an arbitrary suffix of the original PTY byte stream.
    pub fn ansi_snapshot(&self) -> Vec<u8> {
        let mut bytes = b"\x1bc\x1b[H\x1b[2J\x1b[3J".to_vec();
        if let Some(primary) = self.primary_screen.as_deref() {
            append_plain_screen(&mut bytes, &primary.scrollback, &primary.grid);
            append_saved_screen_state(&mut bytes, primary);
            bytes.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J");
            append_plain_screen(&mut bytes, &self.scrollback, &self.grid);
        } else {
            append_plain_screen(&mut bytes, &self.scrollback, &self.grid);
        }
        self.append_live_state(&mut bytes);
        bytes
    }

    /// Produces a complete ANSI reconstruction within `max_bytes` by dropping
    /// only the oldest scrollback rows. The active viewport, alternate/primary
    /// screens, modes, cursor, and in-flight parser state are preserved.
    pub fn ansi_snapshot_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let full = self.ansi_snapshot();
        if full.len() <= max_bytes {
            return Some(full);
        }
        let scrollback_rows = self.scrollback_rows();
        if scrollback_rows == 0 {
            return None;
        }

        let mut retained = scrollback_rows
            .saturating_mul(max_bytes)
            .checked_div(full.len().max(1))
            .unwrap_or(0)
            .saturating_mul(9)
            / 10;
        retained = retained.min(scrollback_rows.saturating_sub(1));
        let mut candidate = self.clone();
        candidate.remove_oldest_scrollback(scrollback_rows - retained);
        loop {
            let bytes = candidate.ansi_snapshot();
            if bytes.len() <= max_bytes {
                return Some(bytes);
            }
            if retained == 0 {
                return None;
            }
            let next = retained
                .saturating_mul(max_bytes)
                .checked_div(bytes.len().max(1))
                .unwrap_or(0)
                .saturating_mul(9)
                / 10;
            let next = next.min(retained - 1);
            candidate.remove_oldest_scrollback(retained - next);
            retained = next;
        }
    }

    fn scrollback_rows(&self) -> usize {
        self.scrollback.len()
            + self
                .primary_screen
                .as_deref()
                .map_or(0, |primary| primary.scrollback.len())
    }

    fn remove_oldest_scrollback(&mut self, mut rows: usize) {
        if let Some(primary) = self.primary_screen.as_deref_mut() {
            let remove = rows.min(primary.scrollback.len());
            primary.scrollback.drain(..remove);
            rows -= remove;
        }
        let remove = rows.min(self.scrollback.len());
        self.scrollback.drain(..remove);
    }

    fn is_valid(&self) -> bool {
        valid_geometry(self.columns, self.rows)
            && valid_screen_parts(
                self.columns,
                self.rows,
                &self.grid,
                &self.scrollback,
                self.row,
                self.column,
                self.saved,
                self.scroll_top,
                self.scroll_bottom,
                &self.hyperlinks,
            )
            && self.primary_screen.as_deref().is_none_or(|primary| {
                self.scrollback.is_empty()
                    && valid_saved_screen(primary, self.columns, self.rows, &self.hyperlinks)
            })
            && valid_style(self.style)
            && self
                .active_hyperlink
                .is_none_or(|index| usize::from(index) < self.hyperlinks.len())
            && self.hyperlinks.len() <= MAX_HYPERLINKS
            && self
                .hyperlinks
                .iter()
                .try_fold(0usize, |total, url| total.checked_add(url.len()))
                .is_some_and(|total| total <= MAX_HYPERLINK_BYTES)
            && self
                .hyperlinks
                .iter()
                .all(|url| safe_link_url(url).is_some())
            && self.utf8.len() <= 3
            && (self.utf8.is_empty()
                || (matches!(self.parser, Parser::Ground)
                    && std::str::from_utf8(&self.utf8)
                        .is_err_and(|error| error.error_len().is_none())))
            && match &self.parser {
                Parser::Csi(sequence) => sequence.len() <= 128 && sequence.is_ascii(),
                Parser::Osc { sequence, .. } => sequence.len() <= MAX_OSC_BYTES,
                _ => true,
            }
    }

    fn append_live_state(&self, bytes: &mut Vec<u8>) {
        append_modes(
            bytes,
            self.scroll_top,
            self.scroll_bottom,
            self.origin_mode,
            self.auto_wrap,
            self.insert_mode,
        );
        append_cursor_state(
            bytes,
            self.saved.0,
            self.saved.1,
            &self.grid[self.saved.0],
            self.scroll_top,
            self.scroll_bottom,
            self.origin_mode,
            self.auto_wrap,
            self.insert_mode,
        );
        bytes.extend_from_slice(b"\x1b7");
        append_cursor_state(
            bytes,
            self.row,
            self.column,
            &self.grid[self.row],
            self.scroll_top,
            self.scroll_bottom,
            self.origin_mode,
            self.auto_wrap,
            self.insert_mode,
        );
        append_private_mode(bytes, 25, self.cursor_visible);
        append_private_mode(bytes, 2004, self.bracketed_paste);
        append_private_mode(bytes, 2026, self.synchronized_output);
        append_style(bytes, self.style);
        if let Some(url) = self
            .active_hyperlink
            .and_then(|index| self.hyperlinks.get(usize::from(index)))
        {
            bytes.extend_from_slice(b"\x1b]8;;");
            bytes.extend_from_slice(url.as_bytes());
            bytes.extend_from_slice(b"\x1b\\");
        }
        bytes.extend_from_slice(&self.utf8);
        append_parser_state(bytes, &self.parser);
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match &mut self.parser {
                Parser::Ground => match byte {
                    0x1b => self.parser = Parser::Escape,
                    b'\r' => self.column = 0,
                    b'\n' | 0x0b | 0x0c => self.line_feed(),
                    0x08 => self.column = self.column.saturating_sub(1),
                    b'\t' => self.column = ((self.column / 8 + 1) * 8).min(self.columns - 1),
                    0x20..=0x7e => self.put(byte as char),
                    0x80..=0xff => self.feed_utf8(byte),
                    _ => {}
                },
                Parser::Escape => match byte {
                    b'[' => self.parser = Parser::Csi(String::new()),
                    b']' => {
                        self.parser = Parser::Osc {
                            sequence: Vec::new(),
                            escaped: false,
                            overflowed: false,
                        }
                    }
                    b'P' | b'^' | b'_' => self.parser = Parser::String { escaped: false },
                    b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' | b'#' => {
                        self.parser = Parser::Charset;
                    }
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
                    b'D' => {
                        self.line_feed();
                        self.parser = Parser::Ground;
                    }
                    b'E' => {
                        self.column = 0;
                        self.line_feed();
                        self.parser = Parser::Ground;
                    }
                    b'M' => {
                        self.reverse_index();
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
                Parser::Osc {
                    sequence,
                    escaped,
                    overflowed,
                } => {
                    if byte == 0x07 || (*escaped && byte == b'\\') {
                        let sequence = (!*overflowed).then(|| std::mem::take(sequence));
                        self.parser = Parser::Ground;
                        if let Some(sequence) = sequence {
                            self.osc(&sequence);
                        }
                    } else if *escaped {
                        *overflowed = true;
                        *escaped = byte == 0x1b;
                    } else if byte == 0x1b {
                        *escaped = true;
                    } else if sequence.len() < MAX_OSC_BYTES {
                        sequence.push(byte);
                    } else {
                        *overflowed = true;
                    }
                }
                Parser::String { escaped } => {
                    if *escaped && byte == b'\\' {
                        self.parser = Parser::Ground;
                    } else {
                        *escaped = byte == 0x1b;
                    }
                }
                Parser::Charset => self.parser = Parser::Ground,
            }
        }
    }

    pub fn text(&self) -> String {
        self.rendered().text
    }

    pub fn rendered(&self) -> TerminalText {
        self.rendered_inner(false)
    }

    pub fn rendered_with_cursor(&self) -> TerminalText {
        self.rendered_inner(true)
    }

    fn rendered_inner(&self, include_cursor: bool) -> TerminalText {
        let include_cursor = include_cursor && self.cursor_visible && !self.synchronized_output;
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
        let mut links = Vec::new();
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
            let mut link_run: Option<(usize, u16)> = None;
            let mut previous_cell_range = None;
            for (column, cell) in line[..end].iter().enumerate() {
                if cell.width == 0 {
                    if include_cursor && line_index == cursor_line && column == cursor_column {
                        cursor = previous_cell_range.clone();
                    }
                    continue;
                }
                if link_run.is_some_and(|(_, hyperlink)| Some(hyperlink) != cell.hyperlink) {
                    let (start, hyperlink) = link_run.take().expect("terminal link run exists");
                    if let Some(url) = self.hyperlinks.get(usize::from(hyperlink)) {
                        links.push(TerminalLink {
                            range: start..text.len(),
                            url: url.clone(),
                        });
                    }
                }
                if link_run.is_none()
                    && let Some(hyperlink) = cell.hyperlink
                {
                    link_run = Some((text.len(), hyperlink));
                }
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
                for combining in &cell.combining[..usize::from(cell.combining_len)] {
                    text.push(*combining);
                }
                let cell_range = cell_start..text.len();
                if include_cursor && line_index == cursor_line && column == cursor_column {
                    cursor = Some(cell_range.clone());
                }
                previous_cell_range = Some(cell_range);
            }
            if let Some((start, style)) = run
                && style != TerminalStyle::default()
            {
                spans.push(TerminalSpan {
                    range: start..text.len(),
                    style,
                });
            }
            if let Some((start, hyperlink)) = link_run
                && let Some(url) = self.hyperlinks.get(usize::from(hyperlink))
            {
                links.push(TerminalLink {
                    range: start..text.len(),
                    url: url.clone(),
                });
            }
        }
        TerminalText {
            text,
            spans,
            links,
            cursor,
        }
    }

    fn put(&mut self, character: char) {
        let mut width = terminal_character_width(character);
        if width == 0 {
            self.put_combining(character);
            return;
        }
        width = width.min(self.columns);

        if self.column >= self.columns {
            if self.auto_wrap {
                self.column = 0;
                self.line_feed();
            } else {
                self.column = self.columns - 1;
            }
        }
        if width == 2 && self.column + width > self.columns {
            if self.auto_wrap {
                self.column = 0;
                self.line_feed();
            } else {
                width = 1;
            }
        }
        if self.insert_mode {
            self.insert_characters(width);
        }

        let column = self.column;
        for target in column..(column + width).min(self.columns) {
            clear_wide_character(&mut self.grid[self.row], target);
        }
        self.grid[self.row][column] = Cell {
            character,
            style: self.style,
            width: width as u8,
            combining: ['\0'; 2],
            combining_len: 0,
            hyperlink: self.active_hyperlink,
        };
        if width == 2 {
            self.grid[self.row][column + 1] = Cell {
                width: 0,
                hyperlink: self.active_hyperlink,
                ..Cell::default()
            };
        }
        self.column = if self.auto_wrap {
            column + width
        } else {
            (column + width).min(self.columns - 1)
        };
    }

    fn put_combining(&mut self, character: char) {
        if self.column == 0 {
            return;
        }
        let mut column = self.column.min(self.columns) - 1;
        while column > 0 && self.grid[self.row][column].width == 0 {
            column -= 1;
        }
        let cell = &mut self.grid[self.row][column];
        if cell.character == ' ' || cell.width == 0 {
            return;
        }
        let index = usize::from(cell.combining_len);
        if index < cell.combining.len() {
            cell.combining[index] = character;
            cell.combining_len += 1;
        }
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
        if self.row == self.scroll_bottom && self.row >= self.scroll_top {
            self.scroll_up(self.scroll_top, self.scroll_bottom, 1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.scroll_top {
            self.scroll_down(self.scroll_top, self.scroll_bottom, 1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, top: usize, bottom: usize, amount: usize) {
        let amount = amount.min(bottom.saturating_sub(top) + 1);
        for _ in 0..amount {
            let removed = self.grid.remove(top);
            self.grid.insert(bottom, self.blank_line());
            if top == 0 && self.primary_screen.is_none() {
                self.scrollback.push(removed);
            }
        }
        self.trim_scrollback();
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, amount: usize) {
        let amount = amount.min(bottom.saturating_sub(top) + 1);
        for _ in 0..amount {
            self.grid.remove(bottom);
            self.grid.insert(top, self.blank_line());
        }
    }

    fn blank_line(&self) -> Vec<Cell> {
        vec![Cell::default(); self.columns]
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
        self.hyperlinks.clear();
        self.active_hyperlink = None;
        self.bracketed_paste = false;
        self.cursor_visible = true;
        self.synchronized_output = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.origin_mode = false;
        self.auto_wrap = true;
        self.insert_mode = false;
        self.primary_screen = None;
    }

    fn erase_row_range(&mut self, row: usize, start: usize, end: usize) {
        let end = end.min(self.columns);
        for column in start.min(end)..end {
            clear_wide_character(&mut self.grid[row], column);
            self.grid[row][column] = Cell::default();
        }
        sanitize_line(&mut self.grid[row]);
    }

    fn insert_characters(&mut self, amount: usize) {
        let column = self.column.min(self.columns - 1);
        let amount = amount.max(1).min(self.columns - column);
        self.grid[self.row].copy_within(column..self.columns - amount, column + amount);
        self.grid[self.row][column..column + amount].fill(Cell::default());
        sanitize_line(&mut self.grid[self.row]);
    }

    fn delete_characters(&mut self, amount: usize) {
        let column = self.column.min(self.columns - 1);
        let amount = amount.max(1).min(self.columns - column);
        self.grid[self.row].copy_within(column + amount..self.columns, column);
        self.grid[self.row][self.columns - amount..].fill(Cell::default());
        sanitize_line(&mut self.grid[self.row]);
    }

    fn erase_characters(&mut self, amount: usize) {
        let column = self.column.min(self.columns - 1);
        let end = column.saturating_add(amount.max(1)).min(self.columns);
        self.erase_row_range(self.row, column, end);
    }

    fn insert_lines(&mut self, amount: usize) {
        if !(self.scroll_top..=self.scroll_bottom).contains(&self.row) {
            return;
        }
        let amount = amount
            .max(1)
            .min(self.scroll_bottom.saturating_sub(self.row) + 1);
        for _ in 0..amount {
            self.grid.remove(self.scroll_bottom);
            let line = self.blank_line();
            self.grid.insert(self.row, line);
        }
    }

    fn delete_lines(&mut self, amount: usize) {
        if !(self.scroll_top..=self.scroll_bottom).contains(&self.row) {
            return;
        }
        let amount = amount
            .max(1)
            .min(self.scroll_bottom.saturating_sub(self.row) + 1);
        for _ in 0..amount {
            self.grid.remove(self.row);
            let line = self.blank_line();
            self.grid.insert(self.scroll_bottom, line);
        }
    }

    fn cursor_vertical_bounds(&self) -> (usize, usize) {
        if self.origin_mode {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows - 1)
        }
    }

    fn home_cursor(&mut self) {
        self.row = if self.origin_mode { self.scroll_top } else { 0 };
        self.column = 0;
    }

    fn set_scroll_region(&mut self, values: &[usize]) {
        let top = values.first().copied().unwrap_or(0).max(1) - 1;
        let bottom = values.get(1).copied().unwrap_or(0);
        let bottom = if bottom == 0 {
            self.rows - 1
        } else {
            bottom.saturating_sub(1).min(self.rows - 1)
        };
        if top < bottom && top < self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
        }
        self.home_cursor();
    }

    fn set_private_mode(&mut self, mode: usize, enabled: bool) {
        match mode {
            6 => {
                self.origin_mode = enabled;
                self.home_cursor();
            }
            7 => self.auto_wrap = enabled,
            25 => self.cursor_visible = enabled,
            47 | 1047 | 1049 if enabled => self.enter_alternate_screen(),
            47 | 1047 | 1049 => self.leave_alternate_screen(),
            2004 => self.bracketed_paste = enabled,
            2026 => self.synchronized_output = enabled,
            _ => {}
        }
    }

    fn enter_alternate_screen(&mut self) {
        if self.primary_screen.is_some() {
            return;
        }
        let primary = SavedScreen {
            grid: std::mem::take(&mut self.grid),
            scrollback: std::mem::take(&mut self.scrollback),
            row: self.row,
            column: self.column,
            saved: self.saved,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            origin_mode: self.origin_mode,
            auto_wrap: self.auto_wrap,
            insert_mode: self.insert_mode,
        };
        self.primary_screen = Some(Box::new(primary));
        self.grid = vec![vec![Cell::default(); self.columns]; self.rows];
        self.row = 0;
        self.column = 0;
        self.saved = (0, 0);
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.origin_mode = false;
        self.insert_mode = false;
    }

    fn leave_alternate_screen(&mut self) {
        let Some(primary) = self.primary_screen.take() else {
            return;
        };
        self.grid = primary.grid;
        self.scrollback = primary.scrollback;
        self.row = primary.row.min(self.rows - 1);
        self.column = primary.column.min(self.columns);
        self.saved = (
            primary.saved.0.min(self.rows - 1),
            primary.saved.1.min(self.columns - 1),
        );
        self.scroll_top = primary.scroll_top.min(self.rows - 1);
        self.scroll_bottom = primary.scroll_bottom.min(self.rows - 1);
        self.origin_mode = primary.origin_mode;
        self.auto_wrap = primary.auto_wrap;
        self.insert_mode = primary.insert_mode;
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            1 => {
                for row in 0..self.row {
                    self.grid[row].fill(Cell::default());
                }
                self.erase_row_range(self.row, 0, self.column.min(self.columns - 1) + 1);
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
                self.erase_row_range(self.row, self.column.min(self.columns - 1), self.columns);
                for row in self.row + 1..self.rows {
                    self.grid[row].fill(Cell::default());
                }
            }
        }
    }

    fn osc(&mut self, sequence: &[u8]) {
        let Ok(sequence) = std::str::from_utf8(sequence) else {
            return;
        };
        let Some(payload) = sequence.strip_prefix("8;") else {
            return;
        };
        let Some((_, url)) = payload.split_once(';') else {
            self.active_hyperlink = None;
            return;
        };
        if url.is_empty() {
            self.active_hyperlink = None;
            return;
        }
        let Some(url) = safe_link_url(url) else {
            self.active_hyperlink = None;
            return;
        };
        if let Some(index) = self.hyperlinks.iter().position(|existing| existing == url) {
            self.active_hyperlink = u16::try_from(index).ok();
            return;
        }
        if self.hyperlinks.len() >= MAX_HYPERLINKS
            || self
                .hyperlinks
                .iter()
                .map(String::len)
                .sum::<usize>()
                .saturating_add(url.len())
                > MAX_HYPERLINK_BYTES
        {
            self.active_hyperlink = None;
            return;
        }
        self.hyperlinks.push(url.to_owned());
        self.active_hyperlink = u16::try_from(self.hyperlinks.len() - 1).ok();
    }

    fn csi(&mut self, sequence: &str, command: char) {
        let private = sequence.starts_with('?');
        let secondary = sequence.starts_with('>');
        let sequence = sequence.trim_start_matches(['?', '>']);
        let values = sequence
            .split(';')
            .map(|value| value.parse::<usize>().unwrap_or(0))
            .collect::<Vec<_>>();
        let first = values.first().copied().unwrap_or(0);
        let amount = first.max(1);
        if matches!(command, 'h' | 'l') {
            let enabled = command == 'h';
            if private {
                for mode in &values {
                    self.set_private_mode(*mode, enabled);
                }
            } else if !secondary {
                for mode in &values {
                    if *mode == 4 {
                        self.insert_mode = enabled;
                    }
                }
            }
            return;
        }
        if secondary {
            return;
        }
        match command {
            'A' | 'F' => {
                let (top, _) = self.cursor_vertical_bounds();
                self.row = self.row.saturating_sub(amount).max(top);
                if command == 'F' {
                    self.column = 0;
                }
            }
            'B' | 'E' | 'e' => {
                let (_, bottom) = self.cursor_vertical_bounds();
                self.row = self.row.saturating_add(amount).min(bottom);
                if command == 'E' {
                    self.column = 0;
                }
            }
            'C' | 'a' => self.column = (self.column + amount).min(self.columns - 1),
            'D' => self.column = self.column.saturating_sub(amount),
            'G' | '`' => self.column = amount.saturating_sub(1).min(self.columns - 1),
            'd' => {
                let row = amount.saturating_sub(1);
                self.row = if self.origin_mode {
                    self.scroll_top.saturating_add(row).min(self.scroll_bottom)
                } else {
                    row.min(self.rows - 1)
                };
            }
            'H' | 'f' => {
                let row = values
                    .first()
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .saturating_sub(1);
                self.row = if self.origin_mode {
                    self.scroll_top.saturating_add(row).min(self.scroll_bottom)
                } else {
                    row.min(self.rows - 1)
                };
                self.column = values
                    .get(1)
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .saturating_sub(1)
                    .min(self.columns - 1);
            }
            'J' => self.erase_display(first),
            'K' => match first {
                1 => self.erase_row_range(self.row, 0, self.column.min(self.columns - 1) + 1),
                2 => self.grid[self.row].fill(Cell::default()),
                _ => {
                    self.erase_row_range(self.row, self.column.min(self.columns - 1), self.columns)
                }
            },
            '@' => self.insert_characters(amount),
            'P' => self.delete_characters(amount),
            'X' => self.erase_characters(amount),
            'L' => self.insert_lines(amount),
            'M' => self.delete_lines(amount),
            'S' => self.scroll_up(self.scroll_top, self.scroll_bottom, amount),
            'T' => self.scroll_down(self.scroll_top, self.scroll_bottom, amount),
            'I' => {
                for _ in 0..amount {
                    self.column = ((self.column / 8 + 1) * 8).min(self.columns - 1);
                }
            }
            'Z' => {
                for _ in 0..amount {
                    self.column = self.column.saturating_sub(1);
                    self.column -= self.column % 8;
                }
            }
            'r' => self.set_scroll_region(&values),
            'm' => self.sgr(&values),
            's' => self.saved = (self.row, self.column),
            'u' => {
                self.row = self.saved.0.min(self.rows - 1);
                self.column = self.saved.1.min(self.columns - 1);
            }
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

fn valid_geometry(columns: usize, rows: usize) -> bool {
    (1..=MAX_GEOMETRY).contains(&columns) && (1..=MAX_GEOMETRY).contains(&rows)
}

#[allow(clippy::too_many_arguments)]
fn valid_screen_parts(
    columns: usize,
    rows: usize,
    grid: &[Vec<Cell>],
    scrollback: &[Vec<Cell>],
    row: usize,
    column: usize,
    saved: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    hyperlinks: &[String],
) -> bool {
    grid.len() == rows
        && scrollback.len() <= MAX_SCROLLBACK
        && grid
            .iter()
            .chain(scrollback)
            .all(|line| line.len() == columns && valid_line(line, hyperlinks.len()))
        && row < rows
        && column <= columns
        && saved.0 < rows
        && saved.1 <= columns
        && scroll_top <= scroll_bottom
        && scroll_bottom < rows
}

fn valid_saved_screen(
    screen: &SavedScreen,
    columns: usize,
    rows: usize,
    hyperlinks: &[String],
) -> bool {
    valid_screen_parts(
        columns,
        rows,
        &screen.grid,
        &screen.scrollback,
        screen.row,
        screen.column,
        screen.saved,
        screen.scroll_top,
        screen.scroll_bottom,
        hyperlinks,
    )
}

fn valid_line(line: &[Cell], hyperlink_count: usize) -> bool {
    line.iter().enumerate().all(|(column, cell)| {
        valid_style(cell.style)
            && cell.combining_len <= cell.combining.len() as u8
            && cell
                .hyperlink
                .is_none_or(|index| usize::from(index) < hyperlink_count)
            && match cell.width {
                0 => column > 0 && line[column - 1].width == 2,
                1 => true,
                2 => column + 1 < line.len() && line[column + 1].width == 0,
                _ => false,
            }
    })
}

fn valid_style(style: TerminalStyle) -> bool {
    style.foreground.is_none_or(|color| color <= 0x00ff_ffff)
        && style.background.is_none_or(|color| color <= 0x00ff_ffff)
}

fn append_plain_screen(bytes: &mut Vec<u8>, scrollback: &[Vec<Cell>], grid: &[Vec<Cell>]) {
    let line_count = scrollback.len() + grid.len();
    for (line_index, line) in scrollback.iter().chain(grid).enumerate() {
        let content_end = line
            .iter()
            .rposition(|cell| cell.character != ' ')
            .map_or(0, |index| index + 1);
        for cell in &line[..content_end] {
            if cell.width == 0 {
                continue;
            }
            let mut encoded = [0; 4];
            bytes.extend_from_slice(cell.character.encode_utf8(&mut encoded).as_bytes());
            for combining in &cell.combining[..usize::from(cell.combining_len)] {
                bytes.extend_from_slice(combining.encode_utf8(&mut encoded).as_bytes());
            }
        }
        if line_index + 1 < line_count {
            bytes.extend_from_slice(b"\r\n");
        }
    }
}

fn append_saved_screen_state(bytes: &mut Vec<u8>, screen: &SavedScreen) {
    append_modes(
        bytes,
        screen.scroll_top,
        screen.scroll_bottom,
        screen.origin_mode,
        screen.auto_wrap,
        screen.insert_mode,
    );
    append_cursor_state(
        bytes,
        screen.saved.0,
        screen.saved.1,
        &screen.grid[screen.saved.0],
        screen.scroll_top,
        screen.scroll_bottom,
        screen.origin_mode,
        screen.auto_wrap,
        screen.insert_mode,
    );
    bytes.extend_from_slice(b"\x1b7");
    append_cursor_state(
        bytes,
        screen.row,
        screen.column,
        &screen.grid[screen.row],
        screen.scroll_top,
        screen.scroll_bottom,
        screen.origin_mode,
        screen.auto_wrap,
        screen.insert_mode,
    );
}

fn append_modes(
    bytes: &mut Vec<u8>,
    scroll_top: usize,
    scroll_bottom: usize,
    origin_mode: bool,
    auto_wrap: bool,
    insert_mode: bool,
) {
    bytes.extend_from_slice(format!("\x1b[{};{}r", scroll_top + 1, scroll_bottom + 1).as_bytes());
    append_private_mode(bytes, 6, origin_mode);
    append_private_mode(bytes, 7, auto_wrap);
    bytes.extend_from_slice(if insert_mode { b"\x1b[4h" } else { b"\x1b[4l" });
}

fn append_private_mode(bytes: &mut Vec<u8>, mode: usize, enabled: bool) {
    bytes.extend_from_slice(format!("\x1b[?{mode}{}", if enabled { 'h' } else { 'l' }).as_bytes());
}

fn append_cursor(
    bytes: &mut Vec<u8>,
    row: usize,
    column: usize,
    scroll_top: usize,
    origin_mode: bool,
) {
    let row = if origin_mode {
        row.saturating_sub(scroll_top)
    } else {
        row
    };
    bytes.extend_from_slice(format!("\x1b[{};{}H", row + 1, column + 1).as_bytes());
}

#[allow(clippy::too_many_arguments)]
fn append_cursor_state(
    bytes: &mut Vec<u8>,
    row: usize,
    column: usize,
    line: &[Cell],
    scroll_top: usize,
    scroll_bottom: usize,
    origin_mode: bool,
    auto_wrap: bool,
    insert_mode: bool,
) {
    // DECRC can leave this simplified emulator outside its origin region, but
    // DECOM-relative CUP cannot address that row. Keep the legacy best-effort
    // cursor placement in that rare state instead of repainting another row.
    if column < line.len() || (origin_mode && !(scroll_top..=scroll_bottom).contains(&row)) {
        append_cursor(bytes, row, column, scroll_top, origin_mode);
        return;
    }

    // CUP cannot address the one-past-end cursor state created by filling the
    // final cell of a row. Repaint that same final glyph with wrapping enabled
    // and insertion disabled, then restore both modes. This leaves the screen
    // unchanged while recreating the pending wrap for the next printable byte.
    let mut glyph_column = line.len() - 1;
    while glyph_column > 0 && line[glyph_column].width == 0 {
        glyph_column -= 1;
    }
    let cell = line[glyph_column];
    append_private_mode(bytes, 7, true);
    bytes.extend_from_slice(b"\x1b[4l");
    append_cursor(bytes, row, glyph_column, scroll_top, origin_mode);
    let mut encoded = [0; 4];
    bytes.extend_from_slice(cell.character.encode_utf8(&mut encoded).as_bytes());
    for combining in &cell.combining[..usize::from(cell.combining_len)] {
        bytes.extend_from_slice(combining.encode_utf8(&mut encoded).as_bytes());
    }
    append_private_mode(bytes, 7, auto_wrap);
    bytes.extend_from_slice(if insert_mode { b"\x1b[4h" } else { b"\x1b[4l" });
}

fn append_style(bytes: &mut Vec<u8>, style: TerminalStyle) {
    bytes.extend_from_slice(b"\x1b[0m");
    let mut parameters = Vec::new();
    if style.bold {
        parameters.push("1".to_owned());
    }
    if style.inverse {
        parameters.push("7".to_owned());
    }
    if let Some(color) = style.foreground {
        parameters.push(format!(
            "38;2;{};{};{}",
            (color >> 16) & 0xff,
            (color >> 8) & 0xff,
            color & 0xff
        ));
    }
    if let Some(color) = style.background {
        parameters.push(format!(
            "48;2;{};{};{}",
            (color >> 16) & 0xff,
            (color >> 8) & 0xff,
            color & 0xff
        ));
    }
    if !parameters.is_empty() {
        bytes.extend_from_slice(b"\x1b[");
        bytes.extend_from_slice(parameters.join(";").as_bytes());
        bytes.push(b'm');
    }
}

fn append_parser_state(bytes: &mut Vec<u8>, parser: &Parser) {
    match parser {
        Parser::Ground => {}
        Parser::Escape => bytes.push(0x1b),
        Parser::Csi(sequence) => {
            bytes.extend_from_slice(b"\x1b[");
            bytes.extend(sequence.chars().map(|character| character as u8));
        }
        Parser::Osc {
            sequence,
            escaped,
            overflowed,
        } => {
            bytes.extend_from_slice(b"\x1b]");
            bytes.extend_from_slice(sequence);
            if *overflowed {
                bytes.push(b'x');
            }
            if *escaped {
                bytes.push(0x1b);
            }
        }
        Parser::String { escaped } => {
            bytes.extend_from_slice(b"\x1bP");
            if *escaped {
                bytes.push(0x1b);
            }
        }
        Parser::Charset => bytes.extend_from_slice(b"\x1b("),
    }
}

fn safe_link_url(url: &str) -> Option<&str> {
    const MAX_LINK_BYTES: usize = 2_048;
    if url.len() > MAX_LINK_BYTES
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let remainder = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("mailto:"))?;
    (!remainder.is_empty()).then_some(url)
}

fn resize_saved_screen(screen: &mut SavedScreen, columns: usize, rows: usize) {
    let old_rows = screen.grid.len().max(1);
    let region_was_full = screen.scroll_top == 0 && screen.scroll_bottom + 1 == old_rows;
    for line in &mut screen.grid {
        line.resize(columns, Cell::default());
        sanitize_line(line);
    }
    let rows_above_viewport = screen.row.saturating_sub(rows - 1);
    for _ in 0..rows_above_viewport {
        screen.scrollback.push(screen.grid.remove(0));
    }
    screen.row = screen.row.saturating_sub(rows_above_viewport);
    screen.saved.0 = screen.saved.0.saturating_sub(rows_above_viewport);
    screen.grid.truncate(rows);
    while screen.grid.len() < rows {
        screen.grid.push(vec![Cell::default(); columns]);
    }
    screen.row = screen.row.min(rows - 1);
    screen.column = screen.column.min(columns - 1);
    screen.saved.0 = screen.saved.0.min(rows - 1);
    screen.saved.1 = screen.saved.1.min(columns - 1);
    if region_was_full {
        screen.scroll_top = 0;
        screen.scroll_bottom = rows - 1;
    } else {
        screen.scroll_top = screen.scroll_top.min(rows - 1);
        screen.scroll_bottom = screen.scroll_bottom.min(rows - 1);
        if screen.scroll_top >= screen.scroll_bottom {
            screen.scroll_top = 0;
            screen.scroll_bottom = rows - 1;
        }
    }
    if screen.scrollback.len() > MAX_SCROLLBACK {
        screen
            .scrollback
            .drain(..screen.scrollback.len() - MAX_SCROLLBACK);
    }
}

fn clear_wide_character(line: &mut [Cell], column: usize) {
    if column >= line.len() {
        return;
    }
    match line[column].width {
        0 if column > 0 && line[column - 1].width == 2 => {
            line[column - 1] = Cell::default();
            line[column] = Cell::default();
        }
        2 => {
            line[column] = Cell::default();
            if column + 1 < line.len() && line[column + 1].width == 0 {
                line[column + 1] = Cell::default();
            }
        }
        _ => line[column] = Cell::default(),
    }
}

fn sanitize_line(line: &mut [Cell]) {
    for column in 0..line.len() {
        match line[column].width {
            0 if column == 0 || line[column - 1].width != 2 => {
                line[column] = Cell::default();
            }
            2 if column + 1 >= line.len() || line[column + 1].width != 0 => {
                line[column] = Cell::default();
            }
            0..=2 => {}
            _ => line[column] = Cell::default(),
        }
    }
}

fn terminal_character_width(character: char) -> usize {
    let value = character as u32;
    if is_zero_width(value) {
        0
    } else if is_full_width(value) {
        2
    } else {
        1
    }
}

fn is_zero_width(value: u32) -> bool {
    matches!(
        value,
        0x0300..=0x036f
            | 0x0483..=0x0489
            | 0x0591..=0x05bd
            | 0x05bf
            | 0x05c1..=0x05c2
            | 0x05c4..=0x05c5
            | 0x0610..=0x061a
            | 0x064b..=0x065f
            | 0x0670
            | 0x06d6..=0x06ed
            | 0x0711
            | 0x0730..=0x074a
            | 0x07a6..=0x07b0
            | 0x07eb..=0x07f3
            | 0x0816..=0x082d
            | 0x0859..=0x085b
            | 0x08d3..=0x0902
            | 0x093a
            | 0x093c
            | 0x0941..=0x0948
            | 0x094d
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x206f
            | 0x20d0..=0x20ff
            | 0xfe00..=0xfe0f
            | 0xfe20..=0xfe2f
            | 0x1f3fb..=0x1f3ff
            | 0xe0100..=0xe01ef
    )
}

fn is_full_width(value: u32) -> bool {
    matches!(
        value,
        0x1100..=0x115f
            | 0x231a..=0x231b
            | 0x2329..=0x232a
            | 0x23e9..=0x23ec
            | 0x23f0
            | 0x23f3
            | 0x25fd..=0x25fe
            | 0x2614..=0x2615
            | 0x2648..=0x2653
            | 0x267f
            | 0x2693
            | 0x26a1
            | 0x26aa..=0x26ab
            | 0x26bd..=0x26be
            | 0x26c4..=0x26c5
            | 0x26ce
            | 0x26d4
            | 0x26ea
            | 0x26f2..=0x26f3
            | 0x26f5
            | 0x26fa
            | 0x26fd
            | 0x2705
            | 0x270a..=0x270b
            | 0x2728
            | 0x274c
            | 0x274e
            | 0x2753..=0x2755
            | 0x2757
            | 0x2795..=0x2797
            | 0x27b0
            | 0x27bf
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f004
            | 0x1f0cf
            | 0x1f18e
            | 0x1f191..=0x1f19a
            | 0x1f200..=0x1f202
            | 0x1f210..=0x1f23b
            | 0x1f240..=0x1f248
            | 0x1f250..=0x1f251
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    )
}

impl TerminalStyle {
    fn resolved(mut self) -> Self {
        if self.inverse {
            std::mem::swap(&mut self.foreground, &mut self.background);
            self.inverse = false;
        }
        if self.foreground.is_none()
            && let Some(background) = self.background
        {
            self.foreground = Some(readable_foreground(background));
        }
        self
    }
}

fn readable_foreground(background: u32) -> u32 {
    let red = (background >> 16) & 0xff;
    let green = (background >> 8) & 0xff;
    let blue = background & 0xff;
    let luminance = red * 299 + green * 587 + blue * 114;
    ansi_color(if luminance < 128_000 { 15 } else { 0 })
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
    fn background_only_styles_supply_a_readable_foreground() {
        let mut screen = TerminalScreen::new(20, 2);
        screen.feed(b"\x1b[48;2;30;30;30mdark\x1b[0m \x1b[48;2;250;250;250mlight");

        let rendered = screen.rendered();
        let dark = rendered
            .spans
            .iter()
            .find(|span| &rendered.text[span.range.clone()] == "dark")
            .expect("dark background span");
        let light = rendered
            .spans
            .iter()
            .find(|span| &rendered.text[span.range.clone()] == "light")
            .expect("light background span");

        assert_eq!(dark.style.foreground, Some(ansi_color(15)));
        assert_eq!(light.style.foreground, Some(ansi_color(0)));
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

    #[test]
    fn preserves_osc8_hyperlinks_in_rendered_terminal_text() {
        let mut screen = TerminalScreen::new(40, 2);
        screen.feed(
            b"\x1b]8;id=commit;https://github.com/org/repo/commit/abc\x1b\\commit abc\x1b]8;;\x07 done",
        );

        let rendered = screen.rendered();

        assert_eq!(rendered.text, "commit abc done");
        assert_eq!(rendered.links.len(), 1);
        assert_eq!(
            &rendered.text[rendered.links[0].range.clone()],
            "commit abc"
        );
        assert_eq!(
            rendered.links[0].url,
            "https://github.com/org/repo/commit/abc"
        );
    }

    #[test]
    fn alternate_screen_restores_the_primary_screen_and_cursor() {
        let mut screen = TerminalScreen::new(12, 3);
        screen.feed(b"main");

        screen.feed(b"\x1b[?1049h");
        screen.feed(b"alternate");
        assert_eq!(screen.text(), "alternate");

        screen.feed(b"\x1b[?1049l");
        screen.feed(b"!");
        assert_eq!(screen.text(), "main!");
    }

    #[test]
    fn private_mode_controls_cursor_visibility() {
        let mut screen = TerminalScreen::new(8, 2);
        screen.feed(b"ready");

        screen.feed(b"\x1b[?25l");
        assert_eq!(screen.rendered_with_cursor().cursor, None);

        screen.feed(b"\x1b[?25h");
        assert!(screen.rendered_with_cursor().cursor.is_some());
    }

    #[test]
    fn synchronized_output_hides_the_cursor_until_the_frame_is_committed() {
        let mut screen = TerminalScreen::new(20, 3);
        screen.feed(b"prompt\x1b[1;7H\x1b[?25h");
        assert!(screen.rendered_with_cursor().cursor.is_some());

        // Codex and other TUIs draw their footer before restoring the input
        // cursor. A real terminal does not expose that intermediate cursor.
        screen.feed(b"\x1b[?2026h\x1b[3;1Hfooter\x1b[3;7H");
        assert_eq!(screen.rendered_with_cursor().cursor, None);

        screen.feed(b"\x1b[1;7H\x1b[?2026l");
        let rendered = screen.rendered_with_cursor();
        let cursor = rendered.cursor.expect("committed input cursor");
        assert_eq!(&rendered.text[cursor], " ");
    }

    #[test]
    fn private_mode_tracks_bracketed_paste() {
        let mut screen = TerminalScreen::new(8, 2);
        assert!(!screen.bracketed_paste());

        screen.feed(b"\x1b[?2004h");
        assert!(screen.bracketed_paste());

        screen.feed(b"\x1b[?2004l");
        assert!(!screen.bracketed_paste());
    }

    #[test]
    fn inserts_deletes_and_erases_characters() {
        let mut screen = TerminalScreen::new(10, 2);
        screen.feed(b"abcdef");
        screen.feed(b"\x1b[3G\x1b[2@");
        assert_eq!(screen.text(), "ab  cdef");

        screen.feed(b"\x1b[1P");
        assert_eq!(screen.text(), "ab cdef");

        screen.feed(b"\x1b[3X");
        assert_eq!(screen.text(), "ab   ef");
    }

    #[test]
    fn inserts_and_deletes_lines_inside_the_scroll_region() {
        let mut screen = TerminalScreen::new(4, 5);
        screen.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD\x1b[5;1HE");
        screen.feed(b"\x1b[2;4r\x1b[3;1H\x1b[L");
        assert_eq!(screen.text(), "A\nB\n\nC\nE");

        screen.feed(b"\x1b[M");
        assert_eq!(screen.text(), "A\nB\nC\n\nE");
    }

    #[test]
    fn line_feed_scrolls_only_the_active_region() {
        let mut screen = TerminalScreen::new(4, 5);
        screen.feed(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC\x1b[4;1HD\x1b[5;1HE");
        screen.feed(b"\x1b[2;4r\x1b[4;1H\n");

        assert_eq!(screen.text(), "A\nC\nD\n\nE");
        assert!(screen.scrollback.is_empty());
    }

    #[test]
    fn top_anchored_partial_region_scroll_preserves_scrollback() {
        let mut screen = TerminalScreen::new(8, 6);
        screen
            .feed(b"\x1b[1;1Hone\x1b[2;1Htwo\x1b[3;1Hthree\x1b[4;1Hfour\x1b[5;1Hfive\x1b[6;1Hsix");

        // Inline TUIs leave their composer below a top-anchored scroll region.
        // Rows pushed off the terminal's top still belong in normal scrollback.
        screen.feed(b"\x1b[1;4r\x1b[2S\x1b[r");

        assert_eq!(screen.text(), "one\ntwo\nthree\nfour\n\n\nfive\nsix");
        assert_eq!(screen.scrollback.len(), 2);
    }

    #[test]
    fn origin_mode_addresses_rows_relative_to_the_scroll_region() {
        let mut screen = TerminalScreen::new(5, 5);
        screen.feed(b"\x1b[2;4r\x1b[?6h\x1b[1;1HX");
        assert_eq!(screen.text(), "\nX");

        screen.feed(b"\x1b[99;1HY");
        assert_eq!(screen.text(), "\nX\n\nY");
    }

    #[test]
    fn full_width_and_combining_characters_use_terminal_columns() {
        let mut screen = TerminalScreen::new(8, 2);
        screen.feed("界".as_bytes());
        assert_eq!((screen.row, screen.column), (0, 2));

        screen.feed("e\u{301}".as_bytes());
        assert_eq!((screen.row, screen.column), (0, 3));

        screen.feed(b"X");

        assert_eq!(screen.text(), "界e\u{301}X");
        assert_eq!((screen.row, screen.column), (0, 4));
    }

    #[test]
    fn a_full_width_character_wraps_before_the_last_column() {
        let mut screen = TerminalScreen::new(4, 2);
        screen.feed("abc界".as_bytes());

        assert_eq!(screen.text(), "abc\n界");
        assert_eq!((screen.row, screen.column), (1, 2));
    }

    #[test]
    fn emoji_modifiers_do_not_consume_an_extra_cell() {
        let mut screen = TerminalScreen::new(8, 2);
        screen.feed("👍🏽X".as_bytes());

        assert_eq!(screen.text(), "👍🏽X");
        assert_eq!((screen.row, screen.column), (0, 3));
    }

    #[test]
    fn checkpoint_roundtrip_preserves_partial_parser_state() {
        let mut uninterrupted = TerminalScreen::new(24, 4);
        uninterrupted.feed(b"before \x1b[31;");

        let checkpoint = uninterrupted
            .checkpoint_bytes_bounded(1024 * 1024)
            .expect("checkpoint fits");
        let mut restored =
            TerminalScreen::from_checkpoint_bytes(&checkpoint).expect("valid checkpoint");

        let continuation = b"1mred \x1b[0m\x1b[?2004h> ";
        uninterrupted.feed(continuation);
        restored.feed(continuation);

        assert_eq!(restored.geometry(), uninterrupted.geometry());
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
        assert_eq!(restored.bracketed_paste(), uninterrupted.bracketed_paste());

        let mut uninterrupted = TerminalScreen::new(8, 2);
        uninterrupted.feed(&[0xe2, 0x82]);
        let checkpoint = uninterrupted
            .checkpoint_bytes_bounded(1024 * 1024)
            .expect("checkpoint fits");
        let mut restored =
            TerminalScreen::from_checkpoint_bytes(&checkpoint).expect("valid checkpoint");
        uninterrupted.feed(&[0xac, b'!']);
        restored.feed(&[0xac, b'!']);
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
    }

    #[test]
    fn checkpoint_roundtrip_preserves_alternate_and_primary_screens() {
        let mut uninterrupted = TerminalScreen::new(16, 3);
        uninterrupted.feed(b"primary> \x1b[?1049halt\x1b[2;4H!");

        let checkpoint = uninterrupted
            .checkpoint_bytes_bounded(1024 * 1024)
            .expect("checkpoint fits");
        let mut restored =
            TerminalScreen::from_checkpoint_bytes(&checkpoint).expect("valid checkpoint");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );

        uninterrupted.feed(b"\x1b[?1049lcontinued");
        restored.feed(b"\x1b[?1049lcontinued");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
    }

    #[test]
    fn checkpoint_rejects_corruption_trailing_bytes_and_oversized_input() {
        let screen = TerminalScreen::new(8, 2);
        let checkpoint = screen
            .checkpoint_bytes_bounded(1024 * 1024)
            .expect("checkpoint fits");

        let mut corrupt = checkpoint.clone();
        corrupt[0] ^= 0xff;
        assert!(TerminalScreen::from_checkpoint_bytes(&corrupt).is_none());

        let mut trailing = checkpoint;
        trailing.push(0);
        assert!(TerminalScreen::from_checkpoint_bytes(&trailing).is_none());
        assert!(TerminalScreen::from_checkpoint_bytes(&vec![0; 64 * 1024 * 1024 + 1]).is_none());
    }

    #[test]
    fn checkpoint_rejects_impossible_parser_state() {
        let encode_unchecked = |screen: &TerminalScreen| {
            let mut bytes = CHECKPOINT_HEADER.to_vec();
            bytes.extend(
                bincode::encode_to_vec(screen, bincode::config::standard())
                    .expect("test state encodes"),
            );
            bytes
        };

        let mut screen = TerminalScreen::new(8, 2);
        screen.parser = Parser::Csi("é".to_owned());
        assert!(TerminalScreen::from_checkpoint_bytes(&encode_unchecked(&screen)).is_none());

        screen.parser = Parser::Escape;
        screen.utf8 = vec![0xe2, 0x82];
        assert!(TerminalScreen::from_checkpoint_bytes(&encode_unchecked(&screen)).is_none());
    }

    #[test]
    fn bounded_checkpoint_trims_oldest_scrollback_without_losing_the_viewport() {
        let mut screen = TerminalScreen::new(24, 4);
        for line in 0..500 {
            screen.feed(format!("line {line:04}\r\n").as_bytes());
        }
        screen.feed(b"\x1b[31mcurrent> \x1b[?2004h");
        let visible = screen.grid.clone();
        let cursor = (screen.row, screen.column);

        let checkpoint = screen
            .checkpoint_bytes_bounded(8 * 1024)
            .expect("viewport fits after trimming scrollback");
        assert!(checkpoint.len() <= 8 * 1024);
        let restored =
            TerminalScreen::from_checkpoint_bytes(&checkpoint).expect("valid checkpoint");

        assert_eq!(restored.grid, visible);
        assert_eq!((restored.row, restored.column), cursor);
        assert!(restored.bracketed_paste());
        assert!(restored.scrollback.len() < screen.scrollback.len());
    }

    #[test]
    fn ansi_snapshot_reconstructs_visible_text_cursor_and_modes() {
        let mut screen = TerminalScreen::new(18, 4);
        screen.feed(b"old one\r\nold two\r\n\x1b[31mred\x1b[0m\r\nprompt> ");
        screen.feed(b"\x1b[?2004h\x1b[?25h");

        let mut restored = TerminalScreen::new(18, 4);
        restored.feed(&screen.ansi_snapshot());

        assert_eq!(restored.text(), screen.text());
        assert_eq!((restored.row, restored.column), (screen.row, screen.column));
        assert!(restored.rendered_with_cursor().cursor.is_some());
        assert_eq!(restored.bracketed_paste(), screen.bracketed_paste());
    }

    #[test]
    fn ansi_snapshot_preserves_pending_auto_wrap_for_future_output() {
        let mut uninterrupted = TerminalScreen::new(4, 2);
        uninterrupted.feed(b"abcd");

        let mut restored = TerminalScreen::new(4, 2);
        restored.feed(&uninterrupted.ansi_snapshot());

        uninterrupted.feed(b"X");
        restored.feed(b"X");

        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
    }

    #[test]
    fn ansi_snapshot_preserves_pending_wrap_for_saved_and_primary_screens() {
        let mut uninterrupted = TerminalScreen::new(4, 3);
        uninterrupted.feed(b"abcd\x1b7\r\nnow");
        let mut restored = TerminalScreen::new(4, 3);
        restored.feed(&uninterrupted.ansi_snapshot());

        uninterrupted.feed(b"\x1b8X");
        restored.feed(b"\x1b8X");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );

        let mut uninterrupted = TerminalScreen::new(4, 2);
        uninterrupted.feed(b"abcd\x1b[?1049hWXYZ");
        let mut restored = TerminalScreen::new(4, 2);
        restored.feed(&uninterrupted.ansi_snapshot());

        uninterrupted.feed(b"X\x1b[?1049lY");
        restored.feed(b"X\x1b[?1049lY");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
    }

    #[test]
    fn ansi_snapshot_preserves_pending_wrap_when_modes_change() {
        let mut uninterrupted = TerminalScreen::new(4, 2);
        uninterrupted.feed(b"abcd\x1b[?7l\x1b[4h");
        let mut restored = TerminalScreen::new(4, 2);
        restored.feed(&uninterrupted.ansi_snapshot());

        assert_eq!(restored.column, uninterrupted.column);
        uninterrupted.feed(b"\x1b[?7hX");
        restored.feed(b"\x1b[?7hX");

        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
        assert_eq!(restored.auto_wrap, uninterrupted.auto_wrap);
        assert_eq!(restored.insert_mode, uninterrupted.insert_mode);
    }

    #[test]
    fn ansi_snapshot_preserves_pending_wrap_at_scrolling_boundaries() {
        let mut uninterrupted = TerminalScreen::new(4, 1);
        uninterrupted.feed(b"abcd");
        let mut restored = TerminalScreen::new(4, 1);
        restored.feed(&uninterrupted.ansi_snapshot());

        uninterrupted.feed(b"X");
        restored.feed(b"X");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );

        let mut uninterrupted = TerminalScreen::new(4, 5);
        uninterrupted.feed(b"\x1b[2;4r\x1b[?6h\x1b[3;1Habcd");
        let mut restored = TerminalScreen::new(4, 5);
        restored.feed(&uninterrupted.ansi_snapshot());

        uninterrupted.feed(b"X");
        restored.feed(b"X");
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
        assert_eq!(restored.scrollback, uninterrupted.scrollback);
    }

    #[test]
    fn bounded_ansi_snapshot_drops_only_old_scrollback() {
        let mut screen = TerminalScreen::new(24, 4);
        for line in 0..500 {
            screen.feed(format!("old line {line:04}\r\n").as_bytes());
        }
        screen.feed(b"\x1b[31mcurrent> \x1b[?2004h");

        let snapshot = screen
            .ansi_snapshot_bounded(2 * 1024)
            .expect("the viewport fits");
        assert!(snapshot.len() <= 2 * 1024);
        let mut restored = TerminalScreen::new(24, 4);
        restored.feed(&snapshot);

        assert!(restored.text().contains("current>"));
        assert!(restored.bracketed_paste());
        assert_eq!((restored.row, restored.column), (screen.row, screen.column));
        assert!(restored.rendered_with_cursor().cursor.is_some());
        assert!(restored.scrollback.len() < screen.scrollback.len());
    }
}
