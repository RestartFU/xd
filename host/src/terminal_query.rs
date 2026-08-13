use serde_json::Value;

const MAX_CONTROL_SEQUENCE: usize = 64;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?1;2c";
const DEFAULT_FOREGROUND: u32 = 0xffffff;
const DEFAULT_BACKGROUND: u32 = 0x000000;
const KITTY_KEYBOARD_FLAGS: &[u8] = b"\x1b[?0u";

#[derive(Debug)]
pub(crate) struct TerminalQueryResponder {
    parser: Parser,
    cursor_row: usize,
    cursor_column: usize,
    foreground_reply: Vec<u8>,
    background_reply: Vec<u8>,
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
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_colors(DEFAULT_FOREGROUND, DEFAULT_BACKGROUND)
    }

    pub(crate) fn from_request(request: &Value) -> Self {
        Self::with_colors(
            requested_color(request, "foreground", DEFAULT_FOREGROUND),
            requested_color(request, "background", DEFAULT_BACKGROUND),
        )
    }

    fn with_colors(foreground: u32, background: u32) -> Self {
        Self {
            parser: Parser::default(),
            cursor_row: 1,
            cursor_column: 1,
            foreground_reply: color_reply(10, foreground),
            background_reply: color_reply(11, background),
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
                            self.reply_to_osc(&sequence, &mut replies);
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

    fn reply_to_osc(&self, sequence: &[u8], replies: &mut Vec<u8>) {
        match sequence {
            b"10;?" => replies.extend_from_slice(&self.foreground_reply),
            b"11;?" => replies.extend_from_slice(&self.background_reply),
            _ => {}
        }
    }
}

fn requested_color(request: &Value, name: &str, default: u32) -> u32 {
    request
        .get(name)
        .and_then(Value::as_u64)
        .filter(|color| *color <= u64::from(0xffffff_u32))
        .map_or(default, |color| color as u32)
}

fn color_reply(code: u8, color: u32) -> Vec<u8> {
    let red = ((color >> 16) & 0xff) * 0x101;
    let green = ((color >> 8) & 0xff) * 0x101;
    let blue = (color & 0xff) * 0x101;
    format!("\x1b]{code};rgb:{red:04x}/{green:04x}/{blue:04x}\x1b\\").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn replies_to_dynamic_color_queries_with_the_requested_palette() {
        let mut responder = TerminalQueryResponder::from_request(&json!({
            "foreground": 0x202020,
            "background": 0xfafafa,
        }));

        assert_eq!(
            responder.feed(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\"),
            concat!(
                "\x1b]10;rgb:2020/2020/2020\x1b\\",
                "\x1b]11;rgb:fafa/fafa/fafa\x1b\\"
            )
            .as_bytes()
        );
    }

    #[test]
    fn invalid_requested_colors_keep_the_legacy_dark_palette() {
        let mut responder = TerminalQueryResponder::from_request(&json!({
            "foreground": -1,
            "background": 0x1000000,
        }));

        assert_eq!(
            responder.feed(b"\x1b]10;?\x07\x1b]11;?\x07"),
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
