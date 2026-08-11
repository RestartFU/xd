const MAX_CONTROL_SEQUENCE: usize = 64;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?1;2c";
const FOREGROUND_COLOR: &[u8] = b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\";
const BACKGROUND_COLOR: &[u8] = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
const KITTY_KEYBOARD_FLAGS: &[u8] = b"\x1b[?0u";

#[derive(Debug, Default)]
pub(crate) struct TerminalQueryResponder {
    parser: Parser,
    cursor_row: usize,
    cursor_column: usize,
}

#[derive(Debug, Default)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Csi {
        sequence: Vec<u8>,
        overflowed: bool,
    },
    Osc {
        sequence: Vec<u8>,
        escaped: bool,
        overflowed: bool,
    },
    String {
        escaped: bool,
    },
}

impl TerminalQueryResponder {
    pub(crate) fn new() -> Self {
        Self {
            cursor_row: 1,
            cursor_column: 1,
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn set_cursor_position(&mut self, row: usize, column: usize) {
        self.cursor_row = row.clamp(1, 9_999);
        self.cursor_column = column.clamp(1, 9_999);
    }

    pub(crate) fn feed(&mut self, output: &[u8]) -> Vec<u8> {
        let mut replies = Vec::new();
        for &byte in output {
            let parser = std::mem::take(&mut self.parser);
            self.parser = match parser {
                Parser::Ground => match byte {
                    0x1b => Parser::Escape,
                    0x9b => Self::csi_parser(),
                    0x9d => Self::osc_parser(),
                    0x90 | 0x98 | 0x9e | 0x9f => Parser::String { escaped: false },
                    _ => Parser::Ground,
                },
                Parser::Escape => match byte {
                    b'[' => Self::csi_parser(),
                    b']' => Self::osc_parser(),
                    b'P' | b'X' | b'^' | b'_' => Parser::String { escaped: false },
                    0x1b => Parser::Escape,
                    _ => Parser::Ground,
                },
                Parser::Csi {
                    mut sequence,
                    mut overflowed,
                } => {
                    if byte == 0x1b {
                        Parser::Escape
                    } else if (0x40..=0x7e).contains(&byte) {
                        if !overflowed {
                            self.reply_to_csi(&sequence, byte, &mut replies);
                        }
                        Parser::Ground
                    } else if (0x20..=0x3f).contains(&byte) {
                        if sequence.len() < MAX_CONTROL_SEQUENCE {
                            sequence.push(byte);
                        } else {
                            overflowed = true;
                        }
                        Parser::Csi {
                            sequence,
                            overflowed,
                        }
                    } else {
                        Parser::Ground
                    }
                }
                Parser::Osc {
                    mut sequence,
                    mut escaped,
                    mut overflowed,
                } => {
                    if byte == 0x9c || byte == 0x07 || (escaped && byte == b'\\') {
                        if !overflowed {
                            Self::reply_to_osc(&sequence, &mut replies);
                        }
                        Parser::Ground
                    } else {
                        if escaped {
                            overflowed = true;
                            escaped = byte == 0x1b;
                        } else if byte == 0x1b {
                            escaped = true;
                        } else if sequence.len() < MAX_CONTROL_SEQUENCE {
                            sequence.push(byte);
                        } else {
                            overflowed = true;
                        }
                        Parser::Osc {
                            sequence,
                            escaped,
                            overflowed,
                        }
                    }
                }
                Parser::String { escaped } => {
                    if byte == 0x9c || (escaped && byte == b'\\') {
                        Parser::Ground
                    } else {
                        Parser::String {
                            escaped: byte == 0x1b,
                        }
                    }
                }
            };
        }
        replies
    }

    fn csi_parser() -> Parser {
        Parser::Csi {
            sequence: Vec::new(),
            overflowed: false,
        }
    }

    fn osc_parser() -> Parser {
        Parser::Osc {
            sequence: Vec::new(),
            escaped: false,
            overflowed: false,
        }
    }

    fn reply_to_csi(&self, sequence: &[u8], command: u8, replies: &mut Vec<u8>) {
        match (sequence, command) {
            (b"5", b'n') => replies.extend_from_slice(b"\x1b[0n"),
            (b"6", b'n') => replies.extend_from_slice(
                format!(
                    "\x1b[{};{}R",
                    self.cursor_row.max(1),
                    self.cursor_column.max(1)
                )
                .as_bytes(),
            ),
            (b"" | b"0", b'c') => replies.extend_from_slice(PRIMARY_DEVICE_ATTRIBUTES),
            (b"?", b'u') => replies.extend_from_slice(KITTY_KEYBOARD_FLAGS),
            _ => {}
        }
    }

    fn reply_to_osc(sequence: &[u8], replies: &mut Vec<u8>) {
        match sequence {
            b"10;?" => replies.extend_from_slice(FOREGROUND_COLOR),
            b"11;?" => replies.extend_from_slice(BACKGROUND_COLOR),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_status_and_cursor_reports() {
        let mut responder = TerminalQueryResponder::new();

        assert_eq!(responder.feed(b"\x1b[5n"), b"\x1b[0n");
        assert_eq!(responder.feed(b"\x1b[6n"), b"\x1b[1;1R");
    }

    #[test]
    fn cursor_report_uses_the_configured_one_based_position() {
        let mut responder = TerminalQueryResponder::new();
        responder.set_cursor_position(12, 34);

        assert_eq!(responder.feed(b"\x1b[6n"), b"\x1b[12;34R");
    }

    #[test]
    fn replies_to_both_primary_device_attribute_queries() {
        let mut responder = TerminalQueryResponder::new();

        assert_eq!(responder.feed(b"\x1b[c\x1b[0c"), b"\x1b[?1;2c\x1b[?1;2c");
    }

    #[test]
    fn replies_to_dynamic_color_queries_with_bel_or_st_terminators() {
        let mut responder = TerminalQueryResponder::new();

        assert_eq!(
            responder.feed(b"\x1b]10;?\x07\x1b]11;?\x1b\\"),
            concat!(
                "\x1b]10;rgb:ffff/ffff/ffff\x1b\\",
                "\x1b]11;rgb:0000/0000/0000\x1b\\"
            )
            .as_bytes()
        );
    }

    #[test]
    fn replies_to_the_kitty_keyboard_flags_query() {
        let mut responder = TerminalQueryResponder::new();

        assert_eq!(responder.feed(b"\x1b[?u"), b"\x1b[?0u");
    }

    #[test]
    fn recognizes_queries_split_at_every_byte_boundary() {
        let mut responder = TerminalQueryResponder::new();
        let mut replies = Vec::new();

        for byte in b"\x1b[5n\x1b]11;?\x1b\\\x1b[?u" {
            replies.extend(responder.feed(std::slice::from_ref(byte)));
        }

        assert_eq!(
            replies,
            concat!("\x1b[0n", "\x1b]11;rgb:0000/0000/0000\x1b\\", "\x1b[?0u").as_bytes()
        );
    }

    #[test]
    fn ignores_plain_text_and_query_shaped_text_inside_control_strings() {
        let mut responder = TerminalQueryResponder::new();

        assert!(responder.feed(b"plain [5n and ]10;? text").is_empty());
        assert!(
            responder
                .feed(b"\x1b]0;title \x1b[5n\x07\x1bPpayload \x1b[6n\x1b\\")
                .is_empty()
        );
    }

    #[test]
    fn ignores_non_query_variants_and_recovers_after_oversized_sequences() {
        let mut responder = TerminalQueryResponder::new();
        let mut input = b"\x1b[>c\x1b[?c\x1b[15n\x1b[".to_vec();
        input.extend(std::iter::repeat_n(b'1', 256));
        input.extend_from_slice(b"c\x1b[5n");

        assert_eq!(responder.feed(&input), b"\x1b[0n");
    }

    #[test]
    fn only_an_adjacent_escape_backslash_terminates_an_osc_string() {
        let mut responder = TerminalQueryResponder::new();

        assert_eq!(
            responder.feed(b"\x1b]10;?\x1bX\\\x1b[5n\x1b\\\x1b[6n"),
            b"\x1b[1;1R"
        );
    }
}
