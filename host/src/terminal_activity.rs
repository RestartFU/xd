use xd_terminal::TerminalTitleActivity;

const ESCAPE: u8 = 0x1b;
const BELL: u8 = 0x07;
const MAX_OSC_PAYLOAD: usize = 256;

#[derive(Clone, Copy, Default)]
enum ParseState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    Discard,
    DiscardEscape,
}

/// Extracts semantic agent activity from OSC sequences without retaining
/// arbitrary terminal output. Incomplete sequences may span reads, while an
/// unterminated or hostile sequence can occupy at most `MAX_OSC_PAYLOAD` bytes.
#[derive(Default)]
pub(crate) struct TerminalActivityParser {
    state: ParseState,
    payload: Vec<u8>,
    titles: TerminalTitleActivity,
}

impl TerminalActivityParser {
    pub(crate) fn feed(&mut self, data: &[u8]) -> Vec<bool> {
        let mut activity = Vec::new();
        for &byte in data {
            match self.state {
                ParseState::Ground => match byte {
                    ESCAPE => self.state = ParseState::Escape,
                    _ => {}
                },
                ParseState::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.state = ParseState::Osc;
                    }
                    ESCAPE => {}
                    _ => self.state = ParseState::Ground,
                },
                ParseState::Osc => match byte {
                    BELL => self.finish(&mut activity),
                    ESCAPE => self.state = ParseState::OscEscape,
                    _ => self.push_payload(byte),
                },
                ParseState::OscEscape => match byte {
                    b'\\' => self.finish(&mut activity),
                    BELL => self.finish(&mut activity),
                    ESCAPE => {
                        if self.payload.len() < MAX_OSC_PAYLOAD {
                            self.payload.push(ESCAPE);
                        } else {
                            self.discard();
                        }
                    }
                    _ => {
                        if self.payload.len() <= MAX_OSC_PAYLOAD.saturating_sub(2) {
                            self.payload.extend_from_slice(&[ESCAPE, byte]);
                            self.state = ParseState::Osc;
                        } else {
                            self.discard();
                        }
                    }
                },
                ParseState::Discard => match byte {
                    BELL => self.state = ParseState::Ground,
                    ESCAPE => self.state = ParseState::DiscardEscape,
                    _ => {}
                },
                ParseState::DiscardEscape => match byte {
                    b'\\' | BELL => self.state = ParseState::Ground,
                    ESCAPE => {}
                    _ => self.state = ParseState::Discard,
                },
            }
        }
        activity
    }

    fn push_payload(&mut self, byte: u8) {
        if self.payload.len() < MAX_OSC_PAYLOAD {
            self.payload.push(byte);
        } else {
            self.discard();
        }
    }

    fn finish(&mut self, activity: &mut Vec<bool>) {
        let working = self
            .payload
            .strip_prefix(b"0;")
            .and_then(|title| self.titles.update(title))
            .or_else(|| parse_progress_activity(&self.payload));
        if let Some(working) = working {
            activity.push(working);
        }
        self.payload.clear();
        self.state = ParseState::Ground;
    }

    fn discard(&mut self) {
        self.payload.clear();
        self.state = ParseState::Discard;
    }
}

fn parse_progress_activity(payload: &[u8]) -> Option<bool> {
    let mut fields = payload.split(|byte| *byte == b';');
    if fields.next()? != b"9" || fields.next()? != b"4" {
        return None;
    }
    match fields.next()? {
        b"0" => Some(false),
        b"1" | b"2" | b"3" | b"4" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalActivityParser;

    #[test]
    fn parses_codex_and_claude_activity_across_chunk_boundaries_and_terminators() {
        let mut parser = TerminalActivityParser::default();

        assert!(parser.feed(b"plain\x1b]0;Think").is_empty());
        assert!(parser.feed(b"ing\x1b").is_empty());
        assert_eq!(parser.feed(b"\\"), vec![true]);
        assert_eq!(parser.feed(b"\x1b]0;Ready\x07"), vec![false]);
        assert_eq!(parser.feed(b"\x1b]9;4;3;42\x07"), vec![true]);
        assert_eq!(parser.feed(b"\x1b]9;4;0;\x1b\\"), vec![false]);
    }

    #[test]
    fn parses_current_codex_and_claude_title_activity() {
        let mut parser = TerminalActivityParser::default();

        assert_eq!(parser.feed("\x1b]0;⠋ xd\x07".as_bytes()), vec![true]);
        assert_eq!(parser.feed(b"\x1b]0;xd\x07"), vec![false]);
        assert_eq!(
            parser.feed("\x1b]0;◐ Fix confirmed bugs\x07".as_bytes()),
            vec![true]
        );
        assert_eq!(
            parser.feed("\x1b]0;✳ Fix confirmed bugs\x07".as_bytes()),
            vec![false]
        );
    }

    #[test]
    fn recognized_sequences_survive_every_byte_boundary_and_keep_wire_order() {
        for (sequence, expected) in [
            (b"\x1b]0;Working\x07".as_slice(), true),
            (b"\x1b]0;Ready\x1b\\".as_slice(), false),
            (b"\x1b]9;4;4;100\x07".as_slice(), true),
            (b"\x1b]9;4;0;\x1b\\".as_slice(), false),
        ] {
            for split in 0..=sequence.len() {
                let mut parser = TerminalActivityParser::default();
                let mut updates = parser.feed(&sequence[..split]);
                updates.extend(parser.feed(&sequence[split..]));
                assert_eq!(updates, vec![expected], "split {split} of {sequence:?}");
            }
        }

        let mut parser = TerminalActivityParser::default();
        assert_eq!(
            parser.feed(b"\x1b]0;Working\x07\x1b]0;Ready\x07"),
            vec![true, false]
        );
    }

    #[test]
    fn recognizes_only_supported_activity_values() {
        let mut parser = TerminalActivityParser::default();

        for title in ["Working", "Thinking", "Waiting", "Starting"] {
            assert_eq!(
                parser.feed(format!("\x1b]0;{title}\x07").as_bytes()),
                vec![true]
            );
        }
        for progress in 1..=4 {
            assert_eq!(
                parser.feed(format!("\x1b]9;4;{progress};\x07").as_bytes()),
                vec![true]
            );
        }
        assert!(parser.feed(b"\x1b]0;Repository title\x07").is_empty());
        assert!(parser.feed(b"\x1b]9;4;5;\x07").is_empty());
    }

    #[test]
    fn oversized_sequences_are_discarded_and_the_parser_resynchronizes() {
        let mut parser = TerminalActivityParser::default();
        let mut oversized = b"\x1b]0;".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 4_096));

        assert!(parser.feed(&oversized).is_empty());
        assert_eq!(parser.feed(b"\x07\x1b]0;Ready\x07"), vec![false]);
    }

    #[test]
    fn utf8_continuation_bytes_do_not_start_c1_control_sequences() {
        let mut parser = TerminalActivityParser::default();

        assert!(parser.feed("📝".as_bytes()).is_empty());
        assert_eq!(parser.feed(b"\x1b]0;Working\x07"), vec![true]);
    }
}
