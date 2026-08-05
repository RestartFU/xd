const MAX_SCROLLBACK: usize = 5_000;
const MAX_GEOMETRY: usize = 1_000;

#[derive(Clone)]
pub struct TerminalScreen {
    columns: usize,
    rows: usize,
    grid: Vec<Vec<char>>,
    scrollback: Vec<Vec<char>>,
    row: usize,
    column: usize,
    saved: (usize, usize),
    parser: Parser,
    utf8: Vec<u8>,
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
            grid: vec![vec![' '; columns]; rows],
            scrollback: Vec::new(),
            row: 0,
            column: 0,
            saved: (0, 0),
            parser: Parser::Ground,
            utf8: Vec::new(),
        }
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.clamp(1, MAX_GEOMETRY);
        let rows = rows.clamp(1, MAX_GEOMETRY);
        for line in &mut self.grid {
            line.resize(columns, ' ');
        }
        while self.grid.len() > rows {
            self.scrollback.push(self.grid.remove(0));
        }
        while self.grid.len() < rows {
            self.grid.push(vec![' '; columns]);
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
                        self.clear();
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

    pub fn text(&self) -> String {
        let mut lines = self.scrollback.clone();
        lines.extend(self.grid.clone());
        while lines
            .last()
            .is_some_and(|line| line.iter().all(|character| *character == ' '))
            && lines.len() > 1
        {
            lines.pop();
        }
        lines
            .iter()
            .map(|line| line.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn put(&mut self, character: char) {
        if self.column >= self.columns {
            self.column = 0;
            self.line_feed();
        }
        self.grid[self.row][self.column] = character;
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
            self.grid.push(vec![' '; self.columns]);
            self.trim_scrollback();
        }
    }

    fn trim_scrollback(&mut self) {
        if self.scrollback.len() > MAX_SCROLLBACK {
            self.scrollback
                .drain(..self.scrollback.len() - MAX_SCROLLBACK);
        }
    }

    fn clear(&mut self) {
        self.grid = vec![vec![' '; self.columns]; self.rows];
        self.row = 0;
        self.column = 0;
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
            'J' if first == 2 || first == 3 => self.clear(),
            'J' => {
                for column in self.column..self.columns {
                    self.grid[self.row][column] = ' ';
                }
                for row in self.row + 1..self.rows {
                    self.grid[row].fill(' ');
                }
            }
            'K' if first == 2 => self.grid[self.row].fill(' '),
            'K' if first == 1 => {
                for column in 0..=self.column.min(self.columns - 1) {
                    self.grid[self.row][column] = ' ';
                }
            }
            'K' => {
                for column in self.column..self.columns {
                    self.grid[self.row][column] = ' ';
                }
            }
            's' => self.saved = (self.row, self.column),
            'u' => (self.row, self.column) = self.saved,
            _ => {}
        }
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
}
