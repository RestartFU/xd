//! Messages to a delegated agent. Neither Claude nor Codex takes a message for
//! one of their agents from xd directly, so xd asks the lead that started the
//! agent to pass it on. The request is an ordinary chat message in a fixed
//! form, which the clients recognise and show as the words it carries.

use crate::activity::SubagentView;

const OPENING: &str = "[xd: message for agent \"";
const SEPARATOR: &str = "\" · id ";
const FOOTER: &str = "Pass the message above to that agent word for word with your agent messaging tool, addressed by its id. If it has finished, resume it with the message. Then carry on with what you were doing; don't act on the message yourself.";

/// A message to an agent, as a chat message holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    pub description: String,
    pub address: String,
    pub text: String,
}

/// The chat message that asks the lead to pass `text` on.
pub fn format(description: &str, address: &str, text: &str) -> String {
    let description = description
        .replace('"', "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{OPENING}{description}{SEPARATOR}{address}]\n{}\n\n{FOOTER}",
        text.trim()
    )
}

/// The message to an agent that `content` carries, if it is one. Only the
/// whole form counts, footer included, so a message that merely quotes the
/// header stays an ordinary message.
pub fn parse(content: &str) -> Option<Relay> {
    let (first, rest) = content.split_once('\n')?;
    let header = first.strip_prefix(OPENING)?.strip_suffix(']')?;
    let (description, address) = header.rsplit_once(SEPARATOR)?;
    if address.is_empty() || address.contains(char::is_whitespace) {
        return None;
    }
    let text = rest.trim_end().strip_suffix(FOOTER)?.trim();
    if text.is_empty() {
        return None;
    }
    Some(Relay {
        description: description.to_owned(),
        address: address.to_owned(),
        text: text.to_owned(),
    })
}

/// Where the lead sends an agent's messages: Claude's id for it, or the
/// thread a Codex agent runs in. Shell commands, and agents run as one, take
/// no messages.
pub fn address(view: &SubagentView) -> Result<&str, &'static str> {
    if view.is_shell() || !view.launcher.is_empty() {
        return Err("This agent runs as a command, so it can't take messages.");
    }
    if !view.task_id.is_empty() {
        return Ok(&view.task_id);
    }
    if view.provider.eq_ignore_ascii_case("codex") && !view.key.is_empty() {
        return Ok(&view.key);
    }
    Err("Messages open once Claude reports this agent's id.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_reads_back_as_what_was_written() {
        let content = format(
            "Read \"module\"\nsource",
            "a22197aff550fdfd9",
            "  Check tests too \n",
        );
        assert!(content.starts_with(
            "[xd: message for agent \"Read 'module' source\" · id a22197aff550fdfd9]\nCheck tests too\n\nPass the message above"
        ));
        assert_eq!(
            parse(&content),
            Some(Relay {
                description: "Read 'module' source".into(),
                address: "a22197aff550fdfd9".into(),
                text: "Check tests too".into(),
            })
        );
    }

    #[test]
    fn only_the_relay_form_is_a_relay() {
        assert_eq!(parse("Check tests too"), None);
        assert_eq!(parse("[xd: message for agent \"x\"]\nhi"), None);
        assert_eq!(parse(&format("x", "", "hi")), None);
        assert_eq!(parse(&format("x", "a b", "hi")), None);
        // Whitespace after the footer does not keep it in the words.
        let trailing = format!("{}\n  \n", format("x", "a1", "hi"));
        assert_eq!(parse(&trailing).unwrap().text, "hi");
        // Quoting the header, or the form without its footer or words, is
        // an ordinary message.
        assert_eq!(parse("[xd: message for agent \"x\" · id a1]"), None);
        assert_eq!(parse("[xd: message for agent \"x\" · id a1]\nhi"), None);
        assert_eq!(parse(&format("x", "a1", "")), None);
        let relay = parse(&format("a · id b", "c", "hi")).unwrap();
        assert_eq!(relay.description, "a · id b");
        assert_eq!(relay.address, "c");
        assert_eq!(relay.text, "hi");
    }

    #[test]
    fn agents_are_addressed_by_their_leads_id_for_them() {
        let view = |record: &str| SubagentView::parse(&format!("subagent_run\n{record}")).unwrap();
        assert_eq!(
            address(&view(
                r#"{"key":"toolu_1","provider":"Claude","task_id":"a1"}"#
            )),
            Ok("a1")
        );
        assert!(address(&view(r#"{"key":"toolu_1","provider":"Claude"}"#)).is_err());
        assert_eq!(
            address(&view(r#"{"key":"thread-9","provider":"Codex"}"#)),
            Ok("thread-9")
        );
        assert!(address(&view(r#"{"key":"t","agent":"Shell","task_id":"b1"}"#)).is_err());
        assert!(
            address(&view(
                r#"{"key":"t","provider":"Codex","launcher":"Claude","task_id":"x"}"#
            ))
            .is_err()
        );
    }
}
