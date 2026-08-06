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
    Analysis(Vec<Block>),
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
    pub url: Option<String>,
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
    parse_document(source, false)
}

pub fn parse_assistant(source: &str) -> Document {
    parse_document(source, true)
}

fn parse_document(source: &str, assistant: bool) -> Document {
    let source = final_display_text(source);
    let (source, byte_truncated) = bounded_prefix(&source, MAX_MARKDOWN_BYTES);
    let sections = if assistant {
        assistant_sections(source)
    } else {
        vec![AssistantSection {
            analysis: false,
            text: source.to_owned(),
        }]
    };
    let mut blocks = Vec::new();
    let mut truncated = byte_truncated;
    let mut block_budget = MAX_MARKDOWN_BLOCKS;
    for section in sections {
        if block_budget == 0 {
            truncated = true;
            break;
        }
        let content_budget = block_budget.saturating_sub(usize::from(section.analysis));
        let (section_blocks, section_truncated) = parse_blocks(&section.text, content_budget);
        truncated |= section_truncated;
        let used = section_blocks.len() + usize::from(section.analysis);
        block_budget = block_budget.saturating_sub(used);
        if section.analysis {
            blocks.push(Block::Analysis(section_blocks));
        } else {
            blocks.extend(section_blocks);
        }
    }
    if truncated && block_budget > 0 {
        blocks.push(Block::Paragraph(parse_inline(
            "[Message shortened to keep rendering responsive]",
        )));
    }
    Document { blocks, truncated }
}

fn parse_blocks(source: &str, block_limit: usize) -> (Vec<Block>, bool) {
    let lines = source.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();
    let mut index = 0;

    while index < lines.len() && blocks.len() < block_limit {
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

    let pending_paragraph = !paragraph.is_empty();
    if blocks.len() < block_limit {
        flush_paragraph(&mut blocks, &mut paragraph);
    }
    (
        blocks,
        index < lines.len() || (pending_paragraph && !paragraph.is_empty()),
    )
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

pub fn plain_document(source: &str) -> Document {
    let display = display_text(source);
    let (display, truncated) = bounded_prefix(&display, MAX_MARKDOWN_BYTES);
    Document {
        blocks: vec![Block::Paragraph(InlineText {
            text: display.to_owned(),
            spans: Vec::new(),
        })],
        truncated,
    }
}

pub fn display_text(source: &str) -> String {
    stream_assistant_sections(&final_display_text(source))
}

fn final_display_text(source: &str) -> String {
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

#[derive(Debug)]
struct AssistantSection {
    analysis: bool,
    text: String,
}

fn assistant_sections(source: &str) -> Vec<AssistantSection> {
    let lines = source.split('\n').collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut active: Option<(usize, bool)> = None;
    let mut fenced = false;
    for (index, line) in lines.iter().enumerate() {
        let marker = line.trim_end_matches('\r').trim();
        if marker.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        match marker {
            "<analysis>" | "<summary>" => {
                if active.is_some() {
                    active = None;
                } else {
                    active = Some((index, marker == "<analysis>"));
                }
            }
            "</analysis>" => {
                if let Some((start, true)) = active {
                    blocks.push((start, index, true));
                }
                active = None;
            }
            "</summary>" => {
                if let Some((start, false)) = active {
                    blocks.push((start, index, false));
                }
                active = None;
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        return vec![AssistantSection {
            analysis: false,
            text: source.to_owned(),
        }];
    }
    blocks.sort_by_key(|block| block.0);
    let mut sections = Vec::new();
    let mut cursor = 0;
    for (start, finish, analysis) in blocks {
        if start < cursor {
            continue;
        }
        push_normal_section(&mut sections, lines[cursor..start].join("\n"));
        sections.push(AssistantSection {
            analysis,
            text: lines[start + 1..finish].join("\n"),
        });
        cursor = finish + 1;
    }
    if cursor <= lines.len() {
        push_normal_section(&mut sections, lines[cursor..].join("\n"));
    }
    sections
}

fn push_normal_section(sections: &mut Vec<AssistantSection>, text: String) {
    let text = text.trim();
    if !text.is_empty() {
        sections.push(AssistantSection {
            analysis: false,
            text: text.to_owned(),
        });
    }
}

fn stream_assistant_sections(source: &str) -> String {
    const TAGS: [&str; 4] = ["<analysis>", "</analysis>", "<summary>", "</summary>"];
    let lines = source.split('\n').collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut mode = 0_u8;
    let mut fenced = false;
    for (index, line) in lines.iter().enumerate() {
        let marker = line.trim_end_matches('\r').trim();
        if marker.starts_with("```") {
            fenced = !fenced;
            if mode != 1 {
                output.push(*line);
            }
            continue;
        }
        if !fenced {
            if index + 1 == lines.len()
                && !marker.is_empty()
                && TAGS.iter().any(|tag| tag.starts_with(marker))
            {
                continue;
            }
            match mode {
                0 if marker == "<analysis>" => {
                    mode = 1;
                    continue;
                }
                0 if marker == "<summary>" => {
                    mode = 2;
                    continue;
                }
                1 => {
                    if marker == "</analysis>" {
                        mode = 0;
                    }
                    continue;
                }
                2 if marker == "</summary>" => {
                    mode = 0;
                    continue;
                }
                _ => {}
            }
        }
        if mode != 1 {
            output.push(*line);
        }
    }
    output.join("\n")
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

pub fn language_for_path(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let language = match name.as_str() {
        "dockerfile" => "dockerfile",
        "makefile" | "gnumakefile" => "makefile",
        _ => {
            let extension = name.rsplit_once('.')?.1;
            match extension {
                "go" | "rs" | "py" | "pyw" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "cr"
                | "rb" | "sh" | "bash" | "zsh" | "sql" | "java" | "kt" | "kts" | "c" | "h"
                | "cc" | "cpp" | "cxx" | "hpp" | "swift" | "json" | "jsonc" | "yml" | "yaml"
                | "toml" | "html" | "htm" | "xml" | "lua" => extension,
                _ => return None,
            }
        }
    };
    Some(match language {
        "pyw" => "python".into(),
        "jsx" | "mjs" | "cjs" => "javascript".into(),
        "tsx" => "typescript".into(),
        "cr" => "crystal".into(),
        "rb" => "ruby".into(),
        "bash" => "shell".into(),
        "kt" | "kts" => "kotlin".into(),
        "h" => "c".into(),
        "cc" | "cxx" | "hpp" => "cpp".into(),
        "jsonc" => "json".into(),
        "htm" => "html".into(),
        other => normalize_language(other),
    })
}

pub fn code_spans(language: Option<&str>, source: &str) -> Vec<CodeSpan> {
    let (source, _) = bounded_prefix(source, MAX_MARKDOWN_BYTES);
    highlight_code(language, source)
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

        if let Some(parsed) = parsed {
            let start = text.len();
            text.push_str(parsed.value);
            spans.push(InlineSpan {
                range: start..text.len(),
                kind: parsed.kind,
                url: parsed.url,
            });
            rest = &rest[parsed.consumed..];
        } else {
            let length = rest.chars().next().map(char::len_utf8).unwrap_or(0);
            text.push_str(&rest[..length]);
            rest = &rest[length..];
        }
    }
    InlineText { text, spans }
}

struct ParsedInline<'a> {
    value: &'a str,
    consumed: usize,
    kind: InlineKind,
    url: Option<String>,
}

fn inline_delimited<'a>(
    source: &'a str,
    delimiter: &str,
    kind: InlineKind,
) -> Option<ParsedInline<'a>> {
    let body = &source[delimiter.len()..];
    let end = body.find(delimiter)?;
    if end == 0 {
        return None;
    }
    Some(ParsedInline {
        value: &body[..end],
        consumed: delimiter.len() + end + delimiter.len(),
        kind,
        url: None,
    })
}

fn inline_link(source: &str) -> Option<ParsedInline<'_>> {
    let label_end = source.find("](")?;
    let url_start = label_end + 2;
    let url_end = source[url_start..].find(')')? + url_start;
    if label_end <= 1 || url_end == url_start {
        return None;
    }
    let url = &source[url_start..url_end];
    Some(ParsedInline {
        value: &source[1..label_end],
        consumed: url_end + 1,
        kind: InlineKind::Link,
        url: safe_link_url(url).map(ToOwned::to_owned),
    })
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
        .or_else(|| url.strip_prefix("http://"))?;
    (!remainder.is_empty()).then_some(url)
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
        "python" | "shell" | "ruby" | "yaml" | "toml" | "dockerfile" | "makefile" => "#",
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
        assert_eq!(paragraph.spans[2].url.as_deref(), Some("https://xd.dev"));
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
    fn links_keep_only_bounded_web_destinations() {
        let document = parse(
            "[secure](https://example.com/path) [plain](http://localhost:3000) [file](file:///tmp/x) [script](javascript:alert(1)) [space](https://example.com/a b)",
        );
        let Block::Paragraph(paragraph) = &document.blocks[0] else {
            panic!("paragraph")
        };
        let urls = paragraph
            .spans
            .iter()
            .map(|span| span.url.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            vec![
                Some("https://example.com/path"),
                Some("http://localhost:3000"),
                None,
                None,
                None,
            ]
        );
    }

    #[test]
    fn assistant_sections_collapse_analysis_and_unwrap_summary() {
        let source = "Before\n<analysis>\nprivate reasoning\n</analysis>\n<summary>\nVisible result\n</summary>";
        let document = parse_assistant(source);
        assert_eq!(document.blocks.len(), 3);
        assert!(matches!(document.blocks[0], Block::Paragraph(_)));
        let Block::Analysis(blocks) = &document.blocks[1] else {
            panic!("analysis")
        };
        let Block::Paragraph(reasoning) = &blocks[0] else {
            panic!("analysis paragraph")
        };
        assert_eq!(reasoning.text, "private reasoning");
        let Block::Paragraph(summary) = &document.blocks[2] else {
            panic!("summary")
        };
        assert_eq!(summary.text, "Visible result");
        assert_eq!(display_text(source), "Before\nVisible result");
    }

    #[test]
    fn assistant_section_tags_in_fences_and_user_text_stay_literal() {
        let fenced = "```xml\n<analysis>\nliteral\n</analysis>\n```";
        let document = parse_assistant(fenced);
        assert!(matches!(document.blocks[0], Block::Code(_)));
        let user = parse("<analysis>\nuser example\n</analysis>");
        assert!(
            user.blocks
                .iter()
                .all(|block| !matches!(block, Block::Analysis(_)))
        );
    }

    #[test]
    fn streaming_hides_analysis_and_partial_wrapper_tags() {
        assert_eq!(
            display_text("Visible\n<analysis>\nhidden\n</analysis>\n<summary>\nDone\n</summary>"),
            "Visible\nDone"
        );
        assert_eq!(display_text("Visible\n<anal"), "Visible");
    }

    #[test]
    fn analysis_sections_share_the_global_block_budget() {
        let source = (0..3_000)
            .map(|index| format!("<analysis>\npart {index}\n</analysis>"))
            .collect::<Vec<_>>()
            .join("\n");
        let document = parse_assistant(&source);
        let rendered_blocks = document
            .blocks
            .iter()
            .map(|block| match block {
                Block::Analysis(blocks) => 1 + blocks.len(),
                _ => 1,
            })
            .sum::<usize>();
        assert!(document.truncated);
        assert!(rendered_blocks <= MAX_MARKDOWN_BLOCKS);
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
    fn infers_editor_languages_from_workspace_paths() {
        assert_eq!(language_for_path("cmd/xd/main.go").as_deref(), Some("go"));
        assert_eq!(
            language_for_path("desktop/src/main.rs").as_deref(),
            Some("rust")
        );
        assert_eq!(
            language_for_path("mobile/build.gradle.kts").as_deref(),
            Some("kotlin")
        );
        assert_eq!(
            language_for_path("scripts/install.bash").as_deref(),
            Some("shell")
        );
        assert_eq!(
            language_for_path("vendor/Dockerfile").as_deref(),
            Some("dockerfile")
        );
        assert_eq!(language_for_path("README.md"), None);
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
    fn plain_streaming_documents_defer_markdown_work_without_losing_text() {
        let document = plain_document("A **streaming** reply");
        let Block::Paragraph(paragraph) = &document.blocks[0] else {
            panic!("paragraph")
        };
        assert_eq!(paragraph.text, "A **streaming** reply");
        assert!(paragraph.spans.is_empty());

        let parsed = parse("A **streaming** reply");
        let Block::Paragraph(paragraph) = &parsed.blocks[0] else {
            panic!("paragraph")
        };
        assert_eq!(paragraph.text, "A streaming reply");
        assert_eq!(paragraph.spans.len(), 1);
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
