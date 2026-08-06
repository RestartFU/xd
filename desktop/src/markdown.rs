use std::ops::Range;

pub const MAX_MARKDOWN_BYTES: usize = 512 * 1024;
pub const MAX_MARKDOWN_BLOCKS: usize = 2_048;
pub const MAX_CODE_SPANS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, content: InlineText },
    Paragraph(InlineText),
    Quote(InlineText),
    ListItem { ordered: bool, content: InlineText },
    Rule,
    Code(CodeBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineText {
    pub text: String,
    pub spans: Vec<InlineSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    pub range: Range<usize>,
    pub kind: InlineKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineKind {
    Strong,
    Emphasis,
    Code,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock {
    pub language: Option<String>,
    pub code: String,
    pub spans: Vec<CodeSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSpan {
    pub range: Range<usize>,
    pub kind: CodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Keyword,
    String,
    Comment,
    Number,
}

pub fn parse(source: &str) -> Document {
    let (source, byte_truncated) = bounded_prefix(source, MAX_MARKDOWN_BYTES);
    let lines = source.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();
    let mut index = 0;

    while index < lines.len() && blocks.len() < MAX_MARKDOWN_BLOCKS {
        let line = lines[index];
        if line.trim().is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph);
            index += 1;
            continue;
        }

        if let Some((marker, marker_len, language)) = fence_start(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            index += 1;
            let mut code_lines = Vec::new();
            while index < lines.len() {
                if fence_end(lines[index], marker, marker_len) {
                    index += 1;
                    break;
                }
                code_lines.push(lines[index]);
                index += 1;
            }
            let code = code_lines.join("\n");
            let spans = highlight_code(language.as_deref(), &code);
            blocks.push(Block::Code(CodeBlock {
                language,
                code,
                spans,
            }));
            continue;
        }

        if let Some((level, text)) = heading(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Heading {
                level,
                content: parse_inline(text),
            });
            index += 1;
            continue;
        }

        if is_rule(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::Rule);
            index += 1;
            continue;
        }

        if let Some(text) = quote(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            let mut quote_lines = vec![text];
            index += 1;
            while index < lines.len() {
                let Some(text) = quote(lines[index]) else {
                    break;
                };
                quote_lines.push(text);
                index += 1;
            }
            blocks.push(Block::Quote(parse_inline(&quote_lines.join("\n"))));
            continue;
        }

        if let Some((ordered, text)) = list_item(line) {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(Block::ListItem {
                ordered,
                content: parse_inline(text),
            });
            index += 1;
            continue;
        }

        paragraph.push(line.trim());
        index += 1;
    }

    flush_paragraph(&mut blocks, &mut paragraph);
    let truncated = byte_truncated || index < lines.len();
    if truncated && blocks.len() < MAX_MARKDOWN_BLOCKS {
        blocks.push(Block::Paragraph(parse_inline(
            "[Message shortened to keep rendering responsive]",
        )));
    }
    Document { blocks, truncated }
}

pub fn code_document(language: Option<&str>, source: &str, truncated: bool) -> Document {
    let (source, byte_truncated) = bounded_prefix(source, MAX_MARKDOWN_BYTES);
    let language = language
        .filter(|language| !language.is_empty())
        .map(normalize_language);
    let code = source.to_owned();
    let spans = highlight_code(language.as_deref(), &code);
    Document {
        blocks: vec![Block::Code(CodeBlock {
            language,
            code,
            spans,
        })],
        truncated: truncated || byte_truncated,
    }
}

pub fn display_text(source: &str) -> String {
    let source = display_without_ask_blocks(source);
    let mut output = String::with_capacity(source.len());
    let mut fenced = false;
    for segment in source.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if fence_marker(line) {
            fenced = !fenced;
            output.push_str(segment);
        } else if fenced {
            output.push_str(segment);
        } else {
            output.push_str(&line.replace("<speak>", "").replace("</speak>", ""));
            if segment.ends_with('\n') {
                output.push('\n');
            }
        }
    }
    output
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskContent {
    pub question: String,
    pub options: Vec<String>,
    pub accepts_input: bool,
}

pub fn ask(source: &str) -> Option<AskContent> {
    let mut offset = 0;
    let mut found = None;
    while let Some(relative) = source[offset..].find("<ask>") {
        let opening = offset + relative;
        let body_start = opening + "<ask>".len();
        let Some(relative_close) = source[body_start..].find("</ask>") else {
            break;
        };
        let close = body_start + relative_close;
        if let Some(parsed) = ask_content(&source[body_start..close]) {
            found = Some(parsed);
        }
        offset = close + "</ask>".len();
    }
    found
}

fn display_without_ask_blocks(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut plain = String::new();
    let mut question = None;
    let mut fenced = false;
    for segment in source.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if fence_marker(line) {
            strip_ask_blocks(&plain, &mut output, &mut question);
            plain.clear();
            output.push_str(segment);
            fenced = !fenced;
        } else if fenced {
            output.push_str(segment);
        } else {
            plain.push_str(segment);
        }
    }
    strip_ask_blocks(&plain, &mut output, &mut question);
    if let Some(question) = question {
        while output.chars().last().is_some_and(char::is_whitespace) {
            output.pop();
        }
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&question);
        output.push_str("**");
    }
    output
}

fn strip_ask_blocks(source: &str, output: &mut String, question: &mut Option<String>) {
    let mut offset = 0;
    while let Some(relative) = source[offset..].find("<ask>") {
        let opening = offset + relative;
        let body_start = opening + "<ask>".len();
        let Some(relative_close) = source[body_start..].find("</ask>") else {
            break;
        };
        let close = body_start + relative_close;
        if let Some(parsed) = ask_content(&source[body_start..close]) {
            output.push_str(&source[offset..opening]);
            *question = Some(format!("**{}", parsed.question));
            offset = close + "</ask>".len();
        } else {
            output.push_str(&source[offset..=opening]);
            offset = opening + 1;
        }
    }
    output.push_str(&source[offset..]);
}

fn ask_content(body: &str) -> Option<AskContent> {
    let mut question = Vec::new();
    let mut options = Vec::new();
    let mut accepts_input = false;
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line == "<input>" {
            accepts_input = true;
        } else if let Some(option) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            if options.len() < 6 && !option.trim().is_empty() {
                options.push(option.trim().chars().take(1_000).collect());
            }
        } else if options.is_empty() {
            question.push(line);
        }
    }
    if options.len() < 2 && !accepts_input {
        return None;
    }
    Some(AskContent {
        question: if question.is_empty() {
            "Which one?".into()
        } else {
            question.join(" ").chars().take(2_000).collect()
        },
        options,
        accepts_input,
    })
}

pub fn spoken_text(source: &str) -> Option<String> {
    const MAX_SPEECH_CHARS: usize = 4_000;
    let mut fenced = false;
    let mut collecting = false;
    let mut invalid = false;
    let mut current = String::new();
    let mut snippets = Vec::new();
    for segment in source.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if fence_marker(line) {
            if collecting {
                invalid = true;
            }
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut rest = line;
        loop {
            if collecting {
                let closing = rest.find("</speak>");
                let nested = rest.find("<speak>");
                if nested.is_some_and(|nested| closing.is_none_or(|closing| nested < closing)) {
                    invalid = true;
                    rest = &rest[nested.unwrap_or_default() + "<speak>".len()..];
                    continue;
                }
                if let Some(closing) = closing {
                    current.push_str(&rest[..closing]);
                    if !invalid {
                        let text = current.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !text.is_empty() {
                            snippets.push(text);
                        }
                    }
                    current.clear();
                    collecting = false;
                    invalid = false;
                    rest = &rest[closing + "</speak>".len()..];
                    continue;
                }
                current.push_str(rest);
                current.push('\n');
                if current.chars().count() > MAX_SPEECH_CHARS {
                    invalid = true;
                }
                break;
            }
            let Some(opening) = rest.find("<speak>") else {
                break;
            };
            collecting = true;
            invalid = false;
            rest = &rest[opening + "<speak>".len()..];
        }
    }
    let spoken = snippets.join(" ");
    if spoken.is_empty() {
        None
    } else {
        Some(spoken.chars().take(MAX_SPEECH_CHARS).collect())
    }
}

fn fence_marker(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

fn bounded_prefix(source: &str, limit: usize) -> (&str, bool) {
    if source.len() <= limit {
        return (source, false);
    }
    let mut end = limit;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    (&source[..end], true)
}

fn flush_paragraph(blocks: &mut Vec<Block>, lines: &mut Vec<&str>) {
    if lines.is_empty() || blocks.len() >= MAX_MARKDOWN_BLOCKS {
        lines.clear();
        return;
    }
    blocks.push(Block::Paragraph(parse_inline(&lines.join(" "))));
    lines.clear();
}

fn fence_start(line: &str) -> Option<(char, usize, Option<String>)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let marker_len = trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if marker_len < 3 {
        return None;
    }
    let info = trimmed[marker_len..].trim();
    let language = info
        .split_whitespace()
        .next()
        .filter(|value| !value.is_empty())
        .map(normalize_language);
    Some((marker, marker_len, language))
}

fn fence_end(line: &str, marker: char, marker_len: usize) -> bool {
    let trimmed = line.trim();
    trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count()
        >= marker_len
        && trimmed.chars().all(|character| character == marker)
}

fn normalize_language(language: &str) -> String {
    let language = language
        .trim_matches(|character| character == '{' || character == '}')
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match language.as_str() {
        "golang" => "go".into(),
        "rs" => "rust".into(),
        "py" => "python".into(),
        "js" => "javascript".into(),
        "ts" => "typescript".into(),
        "sh" | "zsh" => "shell".into(),
        "yml" => "yaml".into(),
        "c++" => "cpp".into(),
        other => other.into(),
    }
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    let count = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&count) || trimmed.as_bytes().get(count) != Some(&b' ') {
        return None;
    }
    Some((count as u8, trimmed[count + 1..].trim()))
}

fn quote(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix('>')
        .map(|text| text.strip_prefix(' ').unwrap_or(text))
}

fn list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(marker) {
            return Some((false, text));
        }
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && trimmed.get(digits..digits + 2) == Some(". ") {
        return Some((true, &trimmed[digits + 2..]));
    }
    None
}

fn is_rule(line: &str) -> bool {
    let compact = line.trim().replace(' ', "");
    compact.len() >= 3
        && compact.chars().next().is_some_and(|marker| {
            matches!(marker, '-' | '*' | '_') && compact.chars().all(|c| c == marker)
        })
}

fn parse_inline(source: &str) -> InlineText {
    let mut text = String::with_capacity(source.len());
    let mut spans = Vec::new();
    let mut rest = source;

    while !rest.is_empty() {
        let Some((offset, marker)) = rest
            .char_indices()
            .find(|(_, character)| matches!(character, '*' | '`' | '['))
        else {
            text.push_str(rest);
            break;
        };
        text.push_str(&rest[..offset]);
        rest = &rest[offset..];

        let parsed = if rest.starts_with("**") {
            inline_delimited(rest, "**", InlineKind::Strong)
        } else if rest.starts_with('*') {
            inline_delimited(rest, "*", InlineKind::Emphasis)
        } else if rest.starts_with('`') {
            inline_delimited(rest, "`", InlineKind::Code)
        } else if marker == '[' {
            inline_link(rest)
        } else {
            None
        };

        if let Some((value, consumed, kind)) = parsed {
            let start = text.len();
            text.push_str(value);
            spans.push(InlineSpan {
                range: start..text.len(),
                kind,
            });
            rest = &rest[consumed..];
        } else {
            let length = rest.chars().next().map(char::len_utf8).unwrap_or(0);
            text.push_str(&rest[..length]);
            rest = &rest[length..];
        }
    }
    InlineText { text, spans }
}

fn inline_delimited<'a>(
    source: &'a str,
    delimiter: &str,
    kind: InlineKind,
) -> Option<(&'a str, usize, InlineKind)> {
    let body = &source[delimiter.len()..];
    let end = body.find(delimiter)?;
    if end == 0 {
        return None;
    }
    Some((&body[..end], delimiter.len() + end + delimiter.len(), kind))
}

fn inline_link(source: &str) -> Option<(&str, usize, InlineKind)> {
    let label_end = source.find("](")?;
    let url_start = label_end + 2;
    let url_end = source[url_start..].find(')')? + url_start;
    if label_end <= 1 || url_end == url_start {
        return None;
    }
    Some((&source[1..label_end], url_end + 1, InlineKind::Link))
}

fn highlight_code(language: Option<&str>, code: &str) -> Vec<CodeSpan> {
    let mut spans = Vec::new();
    let mut index = 0;
    while index < code.len() && spans.len() < MAX_CODE_SPANS {
        let rest = &code[index..];
        if let Some(length) = comment_length(language, rest) {
            spans.push(CodeSpan {
                range: index..index + length,
                kind: CodeKind::Comment,
            });
            index += length;
            continue;
        }

        let character = rest.chars().next().expect("non-empty code remainder");
        if matches!(character, '\'' | '"' | '`') {
            let length = string_length(rest, character);
            spans.push(CodeSpan {
                range: index..index + length,
                kind: CodeKind::String,
            });
            index += length;
            continue;
        }
        if character.is_ascii_digit() {
            let length = rest
                .char_indices()
                .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | 'x'))
                .map(|(offset, c)| offset + c.len_utf8())
                .last()
                .unwrap_or(1);
            spans.push(CodeSpan {
                range: index..index + length,
                kind: CodeKind::Number,
            });
            index += length;
            continue;
        }
        if character.is_ascii_alphabetic() || character == '_' {
            let length = rest
                .char_indices()
                .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
                .map(|(offset, c)| offset + c.len_utf8())
                .last()
                .unwrap_or(1);
            if is_keyword(language, &rest[..length]) {
                spans.push(CodeSpan {
                    range: index..index + length,
                    kind: CodeKind::Keyword,
                });
            }
            index += length;
            continue;
        }
        index += character.len_utf8();
    }
    spans
}

fn comment_length(language: Option<&str>, source: &str) -> Option<usize> {
    let line_comment = match language.unwrap_or_default() {
        "python" | "shell" | "ruby" | "yaml" => "#",
        "sql" | "lua" => "--",
        "html" | "xml" => "<!--",
        _ => "//",
    };
    if source.starts_with(line_comment) {
        return Some(source.find('\n').unwrap_or(source.len()));
    }
    if source.starts_with("/*") {
        return Some(source.find("*/").map_or(source.len(), |end| end + 2));
    }
    None
}

fn string_length(source: &str, quote: char) -> usize {
    let mut escaped = false;
    for (offset, character) in source[quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return quote.len_utf8() + offset + character.len_utf8();
        }
    }
    source.len()
}

fn is_keyword(language: Option<&str>, word: &str) -> bool {
    let words: &[&str] = match language.unwrap_or_default() {
        "go" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
        ],
        "rust" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        "python" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import",
            "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return",
            "True", "try", "while", "with", "yield",
        ],
        "javascript" | "typescript" => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "enum",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "from",
            "function",
            "if",
            "implements",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "with",
            "yield",
        ],
        "crystal" | "ruby" => &[
            "abstract",
            "alias",
            "begin",
            "break",
            "case",
            "class",
            "def",
            "do",
            "else",
            "elsif",
            "end",
            "ensure",
            "enum",
            "extend",
            "false",
            "for",
            "fun",
            "if",
            "include",
            "lib",
            "macro",
            "module",
            "next",
            "nil",
            "of",
            "private",
            "protected",
            "require",
            "rescue",
            "return",
            "self",
            "struct",
            "then",
            "true",
            "unless",
            "until",
            "when",
            "while",
            "with",
            "yield",
        ],
        "shell" => &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
            "in", "local", "readonly", "select", "then", "time", "until", "while",
        ],
        "sql" => &[
            "ALTER", "AND", "AS", "ASC", "BEGIN", "BY", "CASE", "CREATE", "DELETE", "DESC",
            "DISTINCT", "DROP", "ELSE", "END", "FROM", "GROUP", "HAVING", "IN", "INDEX", "INSERT",
            "INTO", "IS", "JOIN", "LIMIT", "NOT", "NULL", "ON", "OR", "ORDER", "SELECT", "SET",
            "TABLE", "THEN", "UNION", "UPDATE", "VALUES", "WHEN", "WHERE",
        ],
        "java" | "kotlin" | "c" | "cpp" | "swift" => &[
            "abstract",
            "auto",
            "bool",
            "break",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extends",
            "false",
            "final",
            "float",
            "for",
            "fun",
            "if",
            "implements",
            "import",
            "in",
            "int",
            "interface",
            "internal",
            "long",
            "namespace",
            "new",
            "null",
            "override",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "static",
            "struct",
            "super",
            "switch",
            "this",
            "throw",
            "throws",
            "true",
            "try",
            "typedef",
            "using",
            "var",
            "virtual",
            "void",
            "when",
            "while",
        ],
        "json" | "yaml" | "toml" => &["false", "null", "true"],
        _ => &[],
    };
    if language == Some("sql") {
        words
            .iter()
            .any(|keyword| word.eq_ignore_ascii_case(keyword))
    } else {
        words.contains(&word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_markdown_blocks_and_inline_styles() {
        let document = parse(
            "# Title\n\nA **bold** and `code` link to [xd](https://xd.dev).\n\n- one\n1. two\n> note\n---",
        );
        assert!(matches!(
            document.blocks[0],
            Block::Heading { level: 1, .. }
        ));
        let Block::Paragraph(paragraph) = &document.blocks[1] else {
            panic!("paragraph")
        };
        assert_eq!(paragraph.text, "A bold and code link to xd.");
        assert_eq!(paragraph.spans.len(), 3);
        assert!(matches!(
            document.blocks[2],
            Block::ListItem { ordered: false, .. }
        ));
        assert!(matches!(
            document.blocks[3],
            Block::ListItem { ordered: true, .. }
        ));
        assert!(matches!(document.blocks[4], Block::Quote(_)));
        assert!(matches!(document.blocks[5], Block::Rule));
    }

    #[test]
    fn highlights_go_fences_by_language() {
        let document =
            parse("```go\npackage main\n// hi\nfunc main() { value := \"ok\"; n := 42 }\n```");
        let Block::Code(code) = &document.blocks[0] else {
            panic!("code")
        };
        assert_eq!(code.language.as_deref(), Some("go"));
        assert!(
            code.spans.iter().any(|span| span.kind == CodeKind::Keyword
                && &code.code[span.range.clone()] == "package")
        );
        assert!(code.spans.iter().any(|span| span.kind == CodeKind::Comment));
        assert!(code.spans.iter().any(|span| span.kind == CodeKind::String));
        assert!(code.spans.iter().any(|span| span.kind == CodeKind::Number));
    }

    #[test]
    fn renders_an_unclosed_streaming_fence_as_code() {
        let document = parse("Before\n\n```py\ndef answer():\n    return 42");
        assert_eq!(document.blocks.len(), 2);
        let Block::Code(code) = &document.blocks[1] else {
            panic!("code")
        };
        assert_eq!(code.language.as_deref(), Some("python"));
        assert_eq!(code.code, "def answer():\n    return 42");
    }

    #[test]
    fn bounds_pathological_messages_and_highlight_counts() {
        let source = format!("```go\n{}", "func value() {}\n".repeat(100_000));
        let document = parse(&source);
        assert!(document.truncated);
        assert!(document.blocks.len() <= MAX_MARKDOWN_BLOCKS);
        let Block::Code(code) = &document.blocks[0] else {
            panic!("code")
        };
        assert!(code.code.len() <= MAX_MARKDOWN_BYTES);
        assert!(code.spans.len() <= MAX_CODE_SPANS);
    }

    #[test]
    fn truncates_on_a_utf8_boundary() {
        let source = "é".repeat(MAX_MARKDOWN_BYTES);
        let document = parse(&source);
        assert!(document.truncated);
    }

    #[test]
    fn speak_tags_are_hidden_and_only_valid_non_code_content_is_selected() {
        let source = "Visible <speak>Say this</speak>.\n```html\n<speak>not this</speak>\n```";
        assert_eq!(
            display_text(source),
            "Visible Say this.\n```html\n<speak>not this</speak>\n```"
        );
        assert_eq!(spoken_text(source).as_deref(), Some("Say this"));
        assert_eq!(spoken_text("<speak>bad <speak>nest</speak>"), None);
        assert_eq!(spoken_text("ordinary reply"), None);
    }

    #[test]
    fn ask_tags_are_hidden_but_questions_remain_in_history() {
        let source = "Ready.\n\n<ask>\nChoose one\n- Fast\n- Safe\n</ask>";
        assert_eq!(display_text(source), "Ready.\n\n**Choose one**");
        assert_eq!(ask(source).unwrap().options, ["Fast", "Safe"]);
        let code = "```text\n<ask>\n- literal\n- code\n</ask>\n```";
        assert_eq!(display_text(code), code);
        assert_eq!(
            display_text("literal <ask>bad</ask>"),
            "literal <ask>bad</ask>"
        );
    }
}
