#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub question: String,
    pub options: Vec<String>,
    pub accepts_input: bool,
}

pub fn parse(text: &str) -> Option<Ask> {
    let mut offset = 0;
    let mut found = None;
    while let Some(relative) = text[offset..].find("<ask>") {
        let opening = offset + relative;
        let body_start = opening + "<ask>".len();
        let Some(relative_close) = text[body_start..].find("</ask>") else {
            break;
        };
        let close = body_start + relative_close;
        if let Some(ask) = parse_body(&text[body_start..close]) {
            found = Some(ask);
        }
        offset = close + "</ask>".len();
    }
    found
}

pub fn visible_bytes(text: &str) -> usize {
    let mut offset = 0;
    while let Some(relative) = text[offset..].find("<ask>") {
        let opening = offset + relative;
        let body_start = opening + "<ask>".len();
        let Some(relative_close) = text[body_start..].find("</ask>") else {
            return opening;
        };
        let close = body_start + relative_close;
        if parse_body(&text[body_start..close]).is_some() {
            return opening;
        }
        offset = opening + 1;
    }
    for length in (1.."<ask>".len()).rev() {
        if text.ends_with(&"<ask>"[..length]) {
            return text.len() - length;
        }
    }
    text.len()
}

fn parse_body(body: &str) -> Option<Ask> {
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
    let question = if question.is_empty() {
        "Which one?".into()
    } else {
        question.join(" ").chars().take(2_000).collect()
    };
    Some(Ask {
        question,
        options,
        accepts_input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_last_valid_question_and_hides_streaming_tags() {
        let text = "Done.\n\n<ask>\nChoose one\n- Fast\n- Safe\n<input>\n</ask>";
        assert_eq!(
            parse(text),
            Some(Ask {
                question: "Choose one".into(),
                options: vec!["Fast".into(), "Safe".into()],
                accepts_input: true,
            })
        );
        assert_eq!(visible_bytes(text), "Done.\n\n".len());
        assert_eq!(visible_bytes("Done.\n<as"), "Done.\n".len());
        assert_eq!(
            visible_bytes("literal <ask>bad</ask>"),
            "literal <ask>bad</ask>".len()
        );
    }
}
