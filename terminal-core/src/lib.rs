use std::ops::Range;

use alacritty_terminal::Grid;
use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb, Timeout};

const MAX_SCROLLBACK: usize = 5_000;
const MAX_GEOMETRY: usize = 1_000;
const MAX_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;
const CHECKPOINT_HEADER: &[u8] = b"XDTS\x02";
const CHECKPOINT_FIXED_BYTES: usize = 5 + 2 + 2 + 4 + 8;
const MAX_PENDING_BYTES: usize = 2 * 1024 * 1024;

/// Converts agent-owned terminal titles into activity transitions. Codex's
/// animated title has no explicit idle token, so the following spinnerless
/// title closes the active interval.
#[derive(Default)]
pub struct TerminalTitleActivity {
    codex_spinner_working: bool,
}

impl TerminalTitleActivity {
    pub fn update(&mut self, title: &[u8]) -> Option<bool> {
        let codex_spinner_working = codex_spinner_title(title);
        let codex_spinner_stopped = self.codex_spinner_working && !codex_spinner_working;
        let update = match title {
            b"Ready" => Some(false),
            title
                if title.starts_with("✳ ".as_bytes())
                    || title.starts_with(b"[ ! ] Action Required")
                    || title.starts_with(b"[ . ] Action Required") =>
            {
                Some(false)
            }
            b"Working" | b"Thinking" | b"Waiting" | b"Starting" => Some(true),
            title
                if title.starts_with("◐ ".as_bytes())
                    || title.starts_with("◑ ".as_bytes())
                    || codex_spinner_working =>
            {
                Some(true)
            }
            _ => codex_spinner_stopped.then_some(false),
        };
        self.codex_spinner_working = codex_spinner_working;
        update
    }
}

fn codex_spinner_title(title: &[u8]) -> bool {
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
        .iter()
        .any(|frame| title.starts_with(frame.as_bytes()))
}

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

#[derive(Clone)]
struct PrimaryState {
    grid: Grid<Cell>,
    mode: TermMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PendingState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc {
        escaped: bool,
    },
    String {
        escaped: bool,
    },
    Charset,
    Utf8,
}

pub struct TerminalScreen {
    columns: usize,
    rows: usize,
    term: Term<VoidListener>,
    processor: Processor,
    primary: Option<PrimaryState>,
    pending: Vec<u8>,
    pending_state: PendingState,
    sync_replay: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct TerminalSize {
    columns: usize,
    rows: usize,
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl TerminalScreen {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.clamp(2, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        let config = Config {
            scrolling_history: MAX_SCROLLBACK,
            ..Config::default()
        };
        let size = TerminalSize { columns, rows };
        Self {
            columns,
            rows,
            term: Term::new(config, &size, VoidListener),
            processor: Processor::new(),
            primary: None,
            pending: Vec::new(),
            pending_state: PendingState::Ground,
            sync_replay: None,
        }
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.clamp(2, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        if self.columns == columns && self.rows == rows {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.term.resize(TerminalSize { columns, rows });
        if let Some(primary) = &mut self.primary {
            primary.grid.resize::<Color>(true, rows, columns);
        }
    }

    pub fn geometry(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn checkpoint_bytes_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let max_bytes = max_bytes.min(MAX_CHECKPOINT_BYTES);
        let ansi_limit = max_bytes.checked_sub(CHECKPOINT_FIXED_BYTES)?;
        let ansi = self.ansi_snapshot_bounded(ansi_limit)?;
        let ansi_len = u32::try_from(ansi.len()).ok()?;
        let columns = u16::try_from(self.columns).ok()?;
        let rows = u16::try_from(self.rows).ok()?;
        let checksum = checkpoint_checksum(columns, rows, &ansi);

        let mut bytes = Vec::with_capacity(CHECKPOINT_FIXED_BYTES + ansi.len());
        bytes.extend_from_slice(CHECKPOINT_HEADER);
        bytes.extend_from_slice(&columns.to_le_bytes());
        bytes.extend_from_slice(&rows.to_le_bytes());
        bytes.extend_from_slice(&ansi_len.to_le_bytes());
        bytes.extend_from_slice(&ansi);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        Some(bytes)
    }

    pub fn from_checkpoint_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < CHECKPOINT_FIXED_BYTES
            || bytes.len() > MAX_CHECKPOINT_BYTES
            || !bytes.starts_with(CHECKPOINT_HEADER)
        {
            return None;
        }
        let mut offset = CHECKPOINT_HEADER.len();
        let columns = read_u16(bytes, &mut offset)?;
        let rows = read_u16(bytes, &mut offset)?;
        let ansi_len = usize::try_from(read_u32(bytes, &mut offset)?).ok()?;
        let ansi_end = offset.checked_add(ansi_len)?;
        let checksum_end = ansi_end.checked_add(8)?;
        if checksum_end != bytes.len() {
            return None;
        }
        let ansi = bytes.get(offset..ansi_end)?;
        offset = ansi_end;
        let checksum = read_u64(bytes, &mut offset)?;
        if checksum != checkpoint_checksum(columns, rows, ansi) {
            return None;
        }
        let columns = usize::from(columns);
        let rows = usize::from(rows);
        if !(2..=MAX_GEOMETRY).contains(&columns) || !(1..=MAX_GEOMETRY).contains(&rows) {
            return None;
        }
        let mut screen = Self::new(columns, rows);
        screen.feed(ansi);
        Some(screen)
    }

    pub fn ansi_snapshot(&self) -> Vec<u8> {
        self.snapshot_with_drop(0)
    }

    pub fn ansi_snapshot_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let full = self.ansi_snapshot();
        if full.len() <= max_bytes {
            return Some(full);
        }
        let history = self.total_history();
        if history == 0 {
            return None;
        }

        let mut dropped = history
            .saturating_sub(history.saturating_mul(max_bytes) / full.len().max(1))
            .max(1);
        loop {
            let bytes = self.snapshot_with_drop(dropped);
            if bytes.len() <= max_bytes {
                return Some(bytes);
            }
            if dropped >= history {
                return None;
            }
            let remaining = history - dropped;
            let keep = remaining.saturating_mul(max_bytes) / bytes.len().max(1);
            dropped = history.saturating_sub(keep).max(dropped + 1).min(history);
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.processor.sync_bytes_count() > 0
                && !self.processor.sync_timeout().pending_timeout()
            {
                self.processor.stop_sync(&mut self.term);
                self.sync_replay = None;
            }

            let alt_before = self.term.mode().contains(TermMode::ALT_SCREEN);
            let sync_before = self.processor.sync_timeout().pending_timeout();
            let entering_alt = !alt_before && self.pending_completes_alt_screen(byte, true);
            let primary = entering_alt.then(|| PrimaryState {
                grid: self.term.grid().clone(),
                mode: *self.term.mode(),
            });
            let sync_prefix =
                (!sync_before && self.pending_completes_sync(byte, true)).then(|| {
                    let mut sequence = self.pending.clone();
                    sequence.push(byte);
                    sequence
                });

            self.processor.advance(&mut self.term, &[byte]);

            let sync_after = self.processor.sync_timeout().pending_timeout();
            match (sync_before, sync_after) {
                (false, true) => self.sync_replay = sync_prefix,
                (true, true) => {
                    if let Some(replay) = &mut self.sync_replay
                        && replay.len() < MAX_PENDING_BYTES
                    {
                        replay.push(byte);
                    }
                }
                (true, false) => self.sync_replay = None,
                (false, false) => {}
            }

            self.track_pending(byte);
            let alt_after = self.term.mode().contains(TermMode::ALT_SCREEN);
            if !alt_before && alt_after {
                self.primary = primary;
            } else if alt_before && !alt_after {
                self.primary = None;
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
        let grid = self.term.grid();
        let include_cursor = include_cursor
            && self.term.mode().contains(TermMode::SHOW_CURSOR)
            && !self.processor.sync_timeout().pending_timeout();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let cursor_line = grid.history_size() as i32 + grid.cursor.point.line.0;
        let cursor_column = grid.cursor.point.column.0.min(self.columns - 1);
        let mut lines = (top..=bottom).map(Line).collect::<Vec<_>>();
        while lines.len() > 1 {
            let index = lines.len() - 1;
            if !(&grid[lines[index]])
                .into_iter()
                .all(cell_is_visually_blank)
                || (include_cursor && index as i32 == cursor_line)
            {
                break;
            }
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
            let row = &grid[line];
            let content_end = row
                .into_iter()
                .rposition(cell_has_visible_content)
                .map_or(0, |i| i + 1);
            let end = if include_cursor && line_index as i32 == cursor_line {
                content_end.max(cursor_column + 1)
            } else {
                content_end
            };
            let mut style_run: Option<(usize, TerminalStyle)> = None;
            let mut link_run: Option<(usize, String)> = None;
            let mut previous_cell_range = None;
            for (column, cell) in row[..Column(end)].iter().enumerate() {
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    if include_cursor && line_index as i32 == cursor_line && column == cursor_column
                    {
                        cursor = previous_cell_range.clone();
                    }
                    continue;
                }
                let link = cell
                    .hyperlink()
                    .and_then(|link| safe_link_url(link.uri()).map(str::to_owned));
                if link_run.as_ref().map(|(_, url)| url) != link.as_ref() {
                    if let Some((start, url)) = link_run.take() {
                        links.push(TerminalLink {
                            range: start..text.len(),
                            url,
                        });
                    }
                    link_run = link.map(|url| (text.len(), url));
                }
                let style = terminal_style(cell);
                if style_run.is_some_and(|(_, current)| current != style) {
                    let (start, current) = style_run.take().expect("terminal style run exists");
                    if current != TerminalStyle::default() {
                        spans.push(TerminalSpan {
                            range: start..text.len(),
                            style: current,
                        });
                    }
                }
                style_run.get_or_insert((text.len(), style));
                let start = text.len();
                text.push(if cell.flags.contains(Flags::HIDDEN) {
                    ' '
                } else {
                    cell.c
                });
                if !cell.flags.contains(Flags::HIDDEN) {
                    for character in cell.zerowidth().into_iter().flatten() {
                        text.push(*character);
                    }
                }
                let range = start..text.len();
                if include_cursor && line_index as i32 == cursor_line && column == cursor_column {
                    cursor = Some(range.clone());
                }
                previous_cell_range = Some(range);
            }
            if let Some((start, style)) = style_run
                && style != TerminalStyle::default()
            {
                spans.push(TerminalSpan {
                    range: start..text.len(),
                    style,
                });
            }
            if let Some((start, url)) = link_run {
                links.push(TerminalLink {
                    range: start..text.len(),
                    url,
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

    fn total_history(&self) -> usize {
        self.term.grid().history_size()
            + self
                .primary
                .as_ref()
                .map_or(0, |primary| primary.grid.history_size())
    }

    fn snapshot_with_drop(&self, dropped: usize) -> Vec<u8> {
        let mut bytes = b"\x1bc\x1b[H\x1b[2J\x1b[3J".to_vec();
        let primary_history = self
            .primary
            .as_ref()
            .map_or(0, |primary| primary.grid.history_size());
        let primary_drop = dropped.min(primary_history);
        let active_drop = dropped.saturating_sub(primary_history);
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);

        if let Some(primary) = &self.primary {
            append_screen(&mut bytes, &primary.grid, primary.mode, primary_drop);
        }
        if alt {
            bytes.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J");
            append_screen(&mut bytes, self.term.grid(), *self.term.mode(), active_drop);
        } else {
            append_screen(&mut bytes, self.term.grid(), *self.term.mode(), active_drop);
        }

        if let Some(sync) = &self.sync_replay {
            bytes.extend_from_slice(sync);
        } else {
            bytes.extend_from_slice(&self.pending);
        }
        bytes
    }

    fn pending_completes_alt_screen(&self, byte: u8, enabled: bool) -> bool {
        if byte != if enabled { b'h' } else { b'l' } || self.pending_state != PendingState::Csi {
            return false;
        }
        let Some(params) = self.pending.strip_prefix(b"\x1b[?") else {
            return false;
        };
        params
            .split(|byte| *byte == b';')
            .any(|mode| matches!(mode, b"47" | b"1047" | b"1049"))
    }

    fn pending_completes_sync(&self, byte: u8, enabled: bool) -> bool {
        byte == if enabled { b'h' } else { b'l' } && self.pending == b"\x1b[?2026"
    }

    fn track_pending(&mut self, byte: u8) {
        use PendingState::*;
        match self.pending_state {
            Ground => match byte {
                0x1b => {
                    self.pending.clear();
                    self.pending.push(byte);
                    self.pending_state = Escape;
                }
                0x80..=0xff => {
                    self.pending.clear();
                    self.pending.push(byte);
                    self.pending_state = Utf8;
                }
                _ => self.pending.clear(),
            },
            Escape => {
                self.push_pending(byte);
                self.pending_state = match byte {
                    b'[' => Csi,
                    b']' => Osc { escaped: false },
                    b'P' | b'^' | b'_' => String { escaped: false },
                    b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' | b'#' => Charset,
                    _ => Ground,
                };
                if self.pending_state == Ground {
                    self.pending.clear();
                }
            }
            Csi => {
                self.push_pending(byte);
                if (0x40..=0x7e).contains(&byte) || self.pending.len() >= MAX_PENDING_BYTES {
                    self.pending.clear();
                    self.pending_state = Ground;
                }
            }
            Osc { escaped } => {
                self.push_pending(byte);
                if byte == 0x07 || (escaped && byte == b'\\') {
                    self.pending.clear();
                    self.pending_state = Ground;
                } else {
                    self.pending_state = Osc {
                        escaped: byte == 0x1b,
                    };
                }
            }
            String { escaped } => {
                self.push_pending(byte);
                if escaped && byte == b'\\' {
                    self.pending.clear();
                    self.pending_state = Ground;
                } else {
                    self.pending_state = String {
                        escaped: byte == 0x1b,
                    };
                }
            }
            Charset => {
                self.pending.clear();
                self.pending_state = Ground;
            }
            Utf8 => {
                self.push_pending(byte);
                match std::str::from_utf8(&self.pending) {
                    Ok(_) => {
                        self.pending.clear();
                        self.pending_state = Ground;
                    }
                    Err(error) if error.error_len().is_some() || self.pending.len() >= 4 => {
                        self.pending.clear();
                        self.pending_state = Ground;
                    }
                    Err(_) => {}
                }
            }
        }
    }

    fn push_pending(&mut self, byte: u8) {
        if self.pending.len() < MAX_PENDING_BYTES {
            self.pending.push(byte);
        } else {
            self.pending.clear();
            self.pending_state = PendingState::Ground;
        }
    }
}

fn append_screen(bytes: &mut Vec<u8>, grid: &Grid<Cell>, mode: TermMode, drop_history: usize) {
    bytes.extend_from_slice(b"\x1b[?6l\x1b[?7h\x1b[4l\x1b[r\x1b[H\x1b[2J\x1b[3J");
    let top = grid.topmost_line().0 + drop_history.min(grid.history_size()) as i32;
    let bottom = grid.bottommost_line().0;
    let mut appearance: Option<CellAppearance> = None;
    let mut hyperlink: Option<String> = None;
    for line_number in top..=bottom {
        let row = &grid[Line(line_number)];
        let wrapped = row[Column(grid.columns() - 1)]
            .flags
            .contains(Flags::WRAPLINE);
        let end = if wrapped {
            grid.columns()
        } else {
            row.into_iter()
                .rposition(cell_has_snapshot_content)
                .map_or(0, |index| index + 1)
        };
        for cell in &row[..Column(end)] {
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let next_link = cell
                .hyperlink()
                .and_then(|link| safe_link_url(link.uri()).map(str::to_owned));
            if next_link != hyperlink {
                append_hyperlink(bytes, next_link.as_deref());
                hyperlink = next_link;
            }
            let next_appearance = CellAppearance::from(cell);
            if appearance.as_ref() != Some(&next_appearance) {
                append_cell_appearance(bytes, &next_appearance);
                appearance = Some(next_appearance);
            }
            append_cell_character(bytes, cell);
        }
        if line_number < bottom && !wrapped {
            bytes.extend_from_slice(b"\r\n");
        }
    }
    if hyperlink.is_some() {
        append_hyperlink(bytes, None);
    }
    bytes.extend_from_slice(b"\x1b[0m");
    append_terminal_state(bytes, grid, mode);
}

fn append_terminal_state(bytes: &mut Vec<u8>, grid: &Grid<Cell>, mode: TermMode) {
    bytes.extend_from_slice(b"\x1b[r\x1b[?6l");
    append_cursor_template(bytes, &grid.saved_cursor.template);
    append_cursor_position(bytes, grid.saved_cursor.point, grid.screen_lines());
    bytes.extend_from_slice(b"\x1b7");

    append_private_mode(bytes, 1, mode.contains(TermMode::APP_CURSOR));
    append_private_mode(bytes, 6, mode.contains(TermMode::ORIGIN));
    append_private_mode(bytes, 7, mode.contains(TermMode::LINE_WRAP));
    append_private_mode(bytes, 25, mode.contains(TermMode::SHOW_CURSOR));
    append_private_mode(bytes, 1004, mode.contains(TermMode::FOCUS_IN_OUT));
    append_private_mode(bytes, 2004, mode.contains(TermMode::BRACKETED_PASTE));
    bytes.extend_from_slice(if mode.contains(TermMode::INSERT) {
        b"\x1b[4h"
    } else {
        b"\x1b[4l"
    });
    bytes.extend_from_slice(if mode.contains(TermMode::APP_KEYPAD) {
        b"\x1b="
    } else {
        b"\x1b>"
    });

    if grid.cursor.input_needs_wrap {
        append_private_mode(bytes, 7, true);
        bytes.extend_from_slice(b"\x1b[4l");
        append_cursor_position(bytes, grid.cursor.point, grid.screen_lines());
        let cell = &grid[grid.cursor.point];
        append_cell_appearance(bytes, &CellAppearance::from(cell));
        append_hyperlink(
            bytes,
            cell.hyperlink()
                .as_ref()
                .and_then(|link| safe_link_url(link.uri())),
        );
        append_cell_character(bytes, cell);
        append_hyperlink(bytes, None);
        append_private_mode(bytes, 7, mode.contains(TermMode::LINE_WRAP));
        bytes.extend_from_slice(if mode.contains(TermMode::INSERT) {
            b"\x1b[4h"
        } else {
            b"\x1b[4l"
        });
    } else {
        append_cursor_position(bytes, grid.cursor.point, grid.screen_lines());
    }
    append_cursor_template(bytes, &grid.cursor.template);
}

fn append_cursor_template(bytes: &mut Vec<u8>, cell: &Cell) {
    append_cell_appearance(bytes, &CellAppearance::from(cell));
    append_hyperlink(
        bytes,
        cell.hyperlink()
            .as_ref()
            .and_then(|link| safe_link_url(link.uri())),
    );
}

fn append_cursor_position(bytes: &mut Vec<u8>, point: Point, rows: usize) {
    let row = point.line.0.clamp(0, rows.saturating_sub(1) as i32) as usize;
    bytes.extend_from_slice(format!("\x1b[{};{}H", row + 1, point.column.0 + 1).as_bytes());
}

fn append_private_mode(bytes: &mut Vec<u8>, mode: usize, enabled: bool) {
    bytes.extend_from_slice(format!("\x1b[?{mode}{}", if enabled { 'h' } else { 'l' }).as_bytes());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellAppearance {
    foreground: Color,
    background: Color,
    flags: Flags,
}

impl From<&Cell> for CellAppearance {
    fn from(cell: &Cell) -> Self {
        Self {
            foreground: cell.fg,
            background: cell.bg,
            flags: cell.flags
                & (Flags::BOLD
                    | Flags::DIM
                    | Flags::ITALIC
                    | Flags::ALL_UNDERLINES
                    | Flags::INVERSE
                    | Flags::HIDDEN
                    | Flags::STRIKEOUT),
        }
    }
}

fn append_cell_appearance(bytes: &mut Vec<u8>, appearance: &CellAppearance) {
    let mut params = vec!["0".to_owned()];
    let flags = appearance.flags;
    if flags.contains(Flags::BOLD) {
        params.push("1".into());
    }
    if flags.contains(Flags::DIM) {
        params.push("2".into());
    }
    if flags.contains(Flags::ITALIC) {
        params.push("3".into());
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        params.push("4".into());
    }
    if flags.contains(Flags::INVERSE) {
        params.push("7".into());
    }
    if flags.contains(Flags::HIDDEN) {
        params.push("8".into());
    }
    if flags.contains(Flags::STRIKEOUT) {
        params.push("9".into());
    }
    append_sgr_color(&mut params, appearance.foreground, true);
    append_sgr_color(&mut params, appearance.background, false);
    bytes.extend_from_slice(b"\x1b[");
    bytes.extend_from_slice(params.join(";").as_bytes());
    bytes.push(b'm');
}

fn append_sgr_color(params: &mut Vec<String>, color: Color, foreground: bool) {
    let default = if foreground { "39" } else { "49" };
    let base = if foreground { 30 } else { 40 };
    match color {
        Color::Spec(rgb) => params.push(format!(
            "{};2;{};{};{}",
            if foreground { 38 } else { 48 },
            rgb.r,
            rgb.g,
            rgb.b
        )),
        Color::Indexed(index) => {
            params.push(format!("{};5;{index}", if foreground { 38 } else { 48 }))
        }
        Color::Named(named) => match named_color_index(named) {
            Some(index @ 0..=7) => params.push((base + index).to_string()),
            Some(index) => {
                params.push(((if foreground { 90 } else { 100 }) + index - 8).to_string())
            }
            None => params.push(default.into()),
        },
    }
}

fn append_hyperlink(bytes: &mut Vec<u8>, url: Option<&str>) {
    bytes.extend_from_slice(b"\x1b]8;;");
    if let Some(url) = url {
        bytes.extend_from_slice(url.as_bytes());
    }
    bytes.extend_from_slice(b"\x1b\\");
}

fn append_cell_character(bytes: &mut Vec<u8>, cell: &Cell) {
    let mut encoded = [0; 4];
    bytes.extend_from_slice(cell.c.encode_utf8(&mut encoded).as_bytes());
    for character in cell.zerowidth().into_iter().flatten() {
        bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
    }
}

fn cell_has_visible_content(cell: &Cell) -> bool {
    !cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        && (cell.c != ' '
            || cell.zerowidth().is_some()
            || cell.hyperlink().is_some()
            || !cell_is_default_appearance(cell))
}

fn cell_has_snapshot_content(cell: &Cell) -> bool {
    cell_has_visible_content(cell) || cell.flags.contains(Flags::WRAPLINE)
}

fn cell_is_visually_blank(cell: &Cell) -> bool {
    !cell_has_visible_content(cell)
}

fn cell_is_default_appearance(cell: &Cell) -> bool {
    cell.fg == Color::Named(NamedColor::Foreground)
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell.flags.intersects(
            Flags::BOLD
                | Flags::DIM
                | Flags::ITALIC
                | Flags::ALL_UNDERLINES
                | Flags::INVERSE
                | Flags::HIDDEN
                | Flags::STRIKEOUT,
        )
}

fn terminal_style(cell: &Cell) -> TerminalStyle {
    TerminalStyle {
        foreground: color_rgb(cell.fg, true),
        background: color_rgb(cell.bg, false),
        bold: cell.flags.contains(Flags::BOLD),
        inverse: cell.flags.contains(Flags::INVERSE),
    }
    .resolved()
}

fn color_rgb(color: Color, foreground: bool) -> Option<u32> {
    match color {
        Color::Spec(Rgb { r, g, b }) => {
            Some((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
        }
        Color::Indexed(index) => Some(xterm_color(usize::from(index))),
        Color::Named(named) => named_color_index(named).map(ansi_color).or_else(|| {
            let is_default = matches!(
                named,
                NamedColor::Foreground
                    | NamedColor::Background
                    | NamedColor::BrightForeground
                    | NamedColor::DimForeground
            );
            (!is_default).then_some(ansi_color(if foreground { 7 } else { 0 }))
        }),
    }
}

fn named_color_index(color: NamedColor) -> Option<usize> {
    match color {
        NamedColor::Black => Some(0),
        NamedColor::Red => Some(1),
        NamedColor::Green => Some(2),
        NamedColor::Yellow => Some(3),
        NamedColor::Blue => Some(4),
        NamedColor::Magenta => Some(5),
        NamedColor::Cyan => Some(6),
        NamedColor::White => Some(7),
        NamedColor::BrightBlack => Some(8),
        NamedColor::BrightRed => Some(9),
        NamedColor::BrightGreen => Some(10),
        NamedColor::BrightYellow => Some(11),
        NamedColor::BrightBlue => Some(12),
        NamedColor::BrightMagenta => Some(13),
        NamedColor::BrightCyan => Some(14),
        NamedColor::BrightWhite => Some(15),
        NamedColor::DimBlack => Some(0),
        NamedColor::DimRed => Some(1),
        NamedColor::DimGreen => Some(2),
        NamedColor::DimYellow => Some(3),
        NamedColor::DimBlue => Some(4),
        NamedColor::DimMagenta => Some(5),
        NamedColor::DimCyan => Some(6),
        NamedColor::DimWhite => Some(7),
        _ => None,
    }
}

fn safe_link_url(url: &str) -> Option<&str> {
    const MAX_LINK_BYTES: usize = 2_048;
    if url.len() > MAX_LINK_BYTES || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let remainder = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("mailto:"))?;
    (!remainder.is_empty()).then_some(url)
}

fn checkpoint_checksum(columns: u16, rows: u16, ansi: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in columns
        .to_le_bytes()
        .into_iter()
        .chain(rows.to_le_bytes())
        .chain(ansi.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let value = u16::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let value = u32::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let value = u64::from_le_bytes(bytes.get(*offset..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
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
    COLORS[index.min(15)]
}

fn xterm_color(index: usize) -> u32 {
    match index.min(255) {
        0..=15 => ansi_color(index),
        16..=231 => {
            const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
            let value = index - 16;
            let red = LEVELS[(value / 36).min(5)];
            let green = LEVELS[((value / 6) % 6).min(5)];
            let blue = LEVELS[(value % 6).min(5)];
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
        screen.feed(b"hello\rworld");
        assert_eq!(screen.text(), "world");
    }

    #[test]
    fn readline_history_deletes_the_old_command_suffix() {
        let mut screen = TerminalScreen::new(24, 3);
        screen.feed(b"$ old command\r\x1b[2K$ new");
        assert_eq!(screen.text(), "$ new");
    }

    #[test]
    fn supports_colors_wide_text_combining_and_links() {
        let mut screen = TerminalScreen::new(40, 4);
        screen.feed(b"\x1b[31mred\x1b[0m \x1b[38;2;1;2;3mX\x1b[0m ");
        screen.feed("界e\u{301}".as_bytes());
        screen.feed(b" \x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
        let rendered = screen.rendered();
        assert!(rendered.text.contains("red X 界e\u{301} link"));
        assert!(rendered.spans.iter().any(|span| {
            &rendered.text[span.range.clone()] == "red"
                && span.style.foreground == Some(ansi_color(1))
        }));
        assert!(rendered.spans.iter().any(|span| {
            &rendered.text[span.range.clone()] == "X" && span.style.foreground == Some(0x010203)
        }));
        assert_eq!(rendered.links.len(), 1);
        assert_eq!(rendered.links[0].url, "https://example.com");
    }

    #[test]
    fn alternate_screen_restores_primary_content() {
        let mut screen = TerminalScreen::new(16, 3);
        screen.feed(b"primary");
        screen.feed(b"\x1b[?1049halternate");
        assert_eq!(screen.text(), "       alternate");
        screen.feed(b"\x1b[?1049l");
        assert_eq!(screen.text(), "primary");
    }

    #[test]
    fn tracks_bracketed_paste() {
        let mut screen = TerminalScreen::new(10, 2);
        assert!(!screen.bracketed_paste());
        screen.feed(b"\x1b[?2004h");
        assert!(screen.bracketed_paste());
        screen.feed(b"\x1b[?2004l");
        assert!(!screen.bracketed_paste());
    }

    #[test]
    fn checkpoint_roundtrip_preserves_state_and_future_output() {
        let mut uninterrupted = TerminalScreen::new(24, 4);
        uninterrupted.feed(b"one\r\ntwo\x1b[31m red\x1b[0m\x1b[?2004h\x1b[");
        let checkpoint = uninterrupted.checkpoint_bytes_bounded(1024 * 1024).unwrap();
        let mut restored = TerminalScreen::from_checkpoint_bytes(&checkpoint).unwrap();
        uninterrupted.feed(b"2Kdone");
        restored.feed(b"2Kdone");
        assert_eq!(restored.rendered(), uninterrupted.rendered());
        assert_eq!(restored.bracketed_paste(), uninterrupted.bracketed_paste());
    }

    #[test]
    fn checkpoint_preserves_split_utf8_and_pending_auto_wrap() {
        let mut uninterrupted = TerminalScreen::new(4, 2);
        uninterrupted.feed(b"abcd");
        uninterrupted.feed(&[0xe2, 0x82]);
        let checkpoint = uninterrupted.checkpoint_bytes_bounded(1024 * 1024).unwrap();
        let mut restored = TerminalScreen::from_checkpoint_bytes(&checkpoint).unwrap();

        uninterrupted.feed(&[0xac, b'!']);
        restored.feed(&[0xac, b'!']);
        assert_eq!(
            restored.rendered_with_cursor(),
            uninterrupted.rendered_with_cursor()
        );
    }

    #[test]
    fn checkpoint_preserves_alternate_and_primary_screens() {
        let mut screen = TerminalScreen::new(16, 3);
        screen.feed(b"primary\x1b[?1049halt");
        let checkpoint = screen.checkpoint_bytes_bounded(1024 * 1024).unwrap();
        let mut restored = TerminalScreen::from_checkpoint_bytes(&checkpoint).unwrap();
        assert_eq!(restored.text(), "       alt");
        restored.feed(b"\x1b[?1049l");
        assert_eq!(restored.text(), "primary");
    }

    #[test]
    fn checkpoint_rejects_corruption_and_trailing_bytes() {
        let screen = TerminalScreen::new(8, 2);
        let checkpoint = screen.checkpoint_bytes_bounded(1024 * 1024).unwrap();
        let mut corrupt = checkpoint.clone();
        corrupt[CHECKPOINT_FIXED_BYTES] ^= 1;
        assert!(TerminalScreen::from_checkpoint_bytes(&corrupt).is_none());
        let mut trailing = checkpoint;
        trailing.push(0);
        assert!(TerminalScreen::from_checkpoint_bytes(&trailing).is_none());
    }

    #[test]
    fn bounded_checkpoint_drops_only_old_scrollback() {
        let mut screen = TerminalScreen::new(24, 4);
        for index in 0..400 {
            screen.feed(format!("line-{index:04}\r\n").as_bytes());
        }
        screen.feed(b"VISIBLE");
        let checkpoint = screen.checkpoint_bytes_bounded(2 * 1024).unwrap();
        assert!(checkpoint.len() <= 2 * 1024);
        let restored = TerminalScreen::from_checkpoint_bytes(&checkpoint).unwrap();
        assert!(restored.text().contains("VISIBLE"));
        assert!(!restored.text().contains("line-0000"));
    }

    #[test]
    fn cursor_visibility_and_synchronized_output_are_respected() {
        let mut screen = TerminalScreen::new(12, 2);
        screen.feed(b"hello");
        assert!(screen.rendered_with_cursor().cursor.is_some());
        screen.feed(b"\x1b[?25l");
        assert!(screen.rendered_with_cursor().cursor.is_none());
        screen.feed(b"\x1b[?25h\x1b[?2026hworld");
        assert_eq!(screen.text(), "hello");
        assert!(screen.rendered_with_cursor().cursor.is_none());
        screen.feed(b"\x1b[?2026l");
        assert!(screen.text().contains("world"));
    }
}
