//! Which of an origin's MCP servers and hooks run on the origin rather than on
//! a runner, and keeping Codex's hook trust true to the origin's.
//!
//! A program whose files a profile cannot carry (a Python environment, a
//! checkout over the size limit, data only the origin has) is *hosted*: the
//! origin keeps its real command under an id, and the runner's configuration
//! starts `~/.xd/origin-exec <id>` instead, which runs it on the origin
//! through `origin_relay`.
//!
//! Codex runs a hook only while `[hooks.state."<key>"] trusted_hash` matches
//! the hook as written. A runner rewrites hook commands (into its profile
//! home, or to `origin-exec`), so it recomputes the hash for exactly the hooks
//! the origin trusts, and leaves the ones the origin does not trust untrusted.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// A program the origin runs for a runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hosted {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    /// What it is, for people: "Claude MCP server src-mcp".
    pub label: String,
}

impl Hosted {
    /// A stable id for the program: the same command gets the same id on
    /// every push, and nothing about it can be read back from the id.
    pub fn id(&self) -> String {
        let identity = json!({
            "program": self.program,
            "args": self.args,
            "env": self.env,
            "cwd": self.cwd,
        });
        crate::origin_profile::sha256(canonical_json(&identity).as_bytes())[..24].to_owned()
    }
}

/// JSON with every object's keys sorted and no spaces, as Codex hashes it.
pub(crate) fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(&map[key])
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical_json).collect::<Vec<_>>().join(",")
        ),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// `PreToolUse` → `pre_tool_use`, as Codex labels events in its trust keys.
pub(crate) fn event_label(event: &str) -> String {
    let mut label = String::new();
    for (index, character) in event.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                label.push('_');
            }
            label.push(character.to_ascii_lowercase());
        } else {
            label.push(character);
        }
    }
    label
}

/// The `sha256:<hex>` Codex trusts a command hook by: its event, its group's
/// matcher, and the handler normalized with Codex's defaults.
pub(crate) fn codex_hook_hash(label: &str, group: &Value, handler: &Value) -> Option<String> {
    if handler["type"].as_str().unwrap_or("command") != "command" {
        return None;
    }
    let mut normalized = Map::new();
    normalized.insert("type".into(), json!("command"));
    normalized.insert("command".into(), handler.get("command")?.clone());
    if let Some(windows) = handler.get("commandWindows").filter(|value| !value.is_null()) {
        normalized.insert("commandWindows".into(), windows.clone());
    }
    let timeout = handler
        .get("timeout")
        .or_else(|| handler.get("timeout_sec"))
        .and_then(Value::as_u64)
        .unwrap_or(600)
        .max(1);
    normalized.insert("timeout".into(), json!(timeout));
    normalized.insert(
        "async".into(),
        json!(handler.get("async").and_then(Value::as_bool).unwrap_or(false)),
    );
    if let Some(message) = handler.get("statusMessage").filter(|value| !value.is_null()) {
        normalized.insert("statusMessage".into(), message.clone());
    }
    let mut identity = Map::new();
    identity.insert("event_name".into(), json!(label));
    if let Some(matcher) = group.get("matcher").filter(|value| !value.is_null()) {
        identity.insert("matcher".into(), matcher.clone());
    }
    identity.insert("hooks".into(), Value::Array(vec![Value::Object(normalized)]));
    let digest = crate::origin_profile::sha256(canonical_json(&Value::Object(identity)).as_bytes());
    Some(format!("sha256:{digest}"))
}

/// The handler a trust key names in `hooks`: `<file>:<event>:<group>:<handler>`.
fn keyed_handler<'a>(hooks: &'a Value, key: &str) -> Option<(String, &'a Value, &'a Value)> {
    let mut parts = key.rsplitn(4, ':');
    let handler_index = parts.next()?.parse::<usize>().ok()?;
    let group_index = parts.next()?.parse::<usize>().ok()?;
    let label = parts.next()?;
    let events = hooks.get("hooks")?.as_object()?;
    let (_, groups) = events.iter().find(|(event, _)| event_label(event) == label)?;
    let group = groups.as_array()?.get(group_index)?;
    let handler = group.get("hooks")?.as_array()?.get(handler_index)?;
    Some((label.to_owned(), group, handler))
}

/// The keys of `hooks_file`'s hooks that `config` trusts as they are written
/// now: the hooks the origin itself runs.
pub(crate) fn trusted_codex_hooks(config: &str, hooks: &str, hooks_file: &str) -> Vec<String> {
    let (Ok(document), Ok(hooks)) = (
        config.parse::<toml_edit::DocumentMut>(),
        serde_json::from_str::<Value>(hooks),
    ) else {
        return Vec::new();
    };
    let Some(state) = document
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml_edit::Item::as_table_like)
    else {
        return Vec::new();
    };
    let prefix = format!("{hooks_file}:");
    let mut trusted = state
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .filter_map(|(key, entry)| {
            let stored = entry.get("trusted_hash")?.as_str()?;
            let (label, group, handler) = keyed_handler(&hooks, key)?;
            (codex_hook_hash(&label, group, handler)? == stored).then(|| key.to_owned())
        })
        .collect::<Vec<_>>();
    trusted.sort();
    trusted
}

/// `config` with `keys` trusted as `hooks` is written now, or `None` when
/// nothing changes.
pub(crate) fn retrust_codex_hooks(config: &str, hooks: &str, keys: &[String]) -> Option<String> {
    let mut document = config.parse::<toml_edit::DocumentMut>().ok()?;
    let hooks = serde_json::from_str::<Value>(hooks).ok()?;
    let mut changed = false;
    for key in keys {
        let Some((label, group, handler)) = keyed_handler(&hooks, key) else {
            continue;
        };
        let Some(hash) = codex_hook_hash(&label, group, handler) else {
            continue;
        };
        if document["hooks"]["state"][key.as_str()]["trusted_hash"].as_str() == Some(hash.as_str()) {
            continue;
        }
        document["hooks"]["state"][key.as_str()]["trusted_hash"] = toml_edit::value(hash);
        changed = true;
    }
    changed.then(|| document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_a_hook_as_codex_does() {
        // Checked against a trust entry Codex 0.171 wrote for this hook.
        let hooks = json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command",
            "command": "/usr/bin/python3 /home/xd/.local/share/cinnabar-internal/dispatch.py",
            "timeout": 30}]}]}});
        let group = &hooks["hooks"]["PreToolUse"][0];
        assert_eq!(
            codex_hook_hash("pre_tool_use", group, &group["hooks"][0]).unwrap(),
            "sha256:e267bb77bf5a668181f26087d2075b426950bc67eba30004122dbfebb3bb76b0"
        );
        assert_eq!(event_label("SubagentStart"), "subagent_start");
    }

    #[test]
    fn carries_trust_only_for_hooks_the_origin_trusts() {
        let origin_hooks = r#"{"hooks": {
            "PreToolUse": [{"hooks": [{"type": "command", "command": "/home/a/guard", "timeout": 30}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "/home/a/stop"}]}]}}"#;
        let hooks: Value = serde_json::from_str(origin_hooks).unwrap();
        let pre = &hooks["hooks"]["PreToolUse"][0];
        let good = codex_hook_hash("pre_tool_use", pre, &pre["hooks"][0]).unwrap();
        let config = format!(
            "model = \"x\"\n\n[hooks.state.\"/home/a/.codex/hooks.json:pre_tool_use:0:0\"]\ntrusted_hash = \"{good}\"\n\n\
             [hooks.state.\"/home/a/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:stale\"\n"
        );
        let trusted = trusted_codex_hooks(&config, origin_hooks, "/home/a/.codex/hooks.json");
        assert_eq!(trusted, vec!["/home/a/.codex/hooks.json:pre_tool_use:0:0"]);

        // On the runner the commands and the key's path have moved.
        let runner_hooks = origin_hooks.replace("/home/a/", "/r/h/");
        let runner_config = config.replace("/home/a/", "/r/h/");
        let keys = vec!["/r/h/.codex/hooks.json:pre_tool_use:0:0".to_owned()];
        let updated = retrust_codex_hooks(&runner_config, &runner_hooks, &keys).unwrap();
        let runner: Value = serde_json::from_str(&runner_hooks).unwrap();
        let pre = &runner["hooks"]["PreToolUse"][0];
        let expected = codex_hook_hash("pre_tool_use", pre, &pre["hooks"][0]).unwrap();
        assert!(updated.contains(&expected), "{updated}");
        assert!(updated.contains("sha256:stale"), "an untrusted hook stays untrusted");
        assert!(updated.starts_with("model = \"x\""));
        assert_eq!(retrust_codex_hooks(&updated, &runner_hooks, &keys), None);
    }

    #[test]
    fn ids_are_stable_and_differ_by_command() {
        let hosted = Hosted {
            program: "/home/a/.venv/bin/srv".into(),
            args: vec!["serve".into()],
            env: BTreeMap::new(),
            cwd: None,
            label: "x".into(),
        };
        let mut other = hosted.clone();
        other.args.push("--debug".into());
        assert_eq!(hosted.id(), hosted.clone().id());
        assert_ne!(hosted.id(), other.id());
        assert!(hosted.id().bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
