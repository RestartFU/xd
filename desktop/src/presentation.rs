//! Display labels for protocol values. Request payloads keep their original IDs.

pub fn model_name(id: &str, catalog_name: Option<&str>) -> String {
    if let Some(name) = catalog_name.filter(|name| !name.trim().is_empty() && *name != id) {
        return name.to_owned();
    }
    match id {
        "" => "Default model",
        "default" | "auto" => "Automatic",
        "gpt-5" => "GPT-5",
        "gpt-5-mini" => "GPT-5 Mini",
        "gpt-5-nano" => "GPT-5 Nano",
        "gpt-5-codex" => "GPT-5 Codex",
        "gpt-5.1" => "GPT-5.1",
        "gpt-5.1-codex" => "GPT-5.1 Codex",
        "gpt-5.1-codex-mini" => "GPT-5.1 Codex Mini",
        "gpt-5.1-codex-max" => "GPT-5.1 Codex Max",
        "gpt-5.2" => "GPT-5.2",
        "gpt-5.2-codex" => "GPT-5.2 Codex",
        "gpt-5.3-codex" => "GPT-5.3 Codex",
        "gpt-5.3-codex-spark" => "GPT-5.3 Codex Spark",
        "gpt-5.4" => "GPT-5.4",
        "gpt-5.4-mini" => "GPT-5.4 Mini",
        "gpt-5.5" => "GPT-5.5",
        "gpt-5.6-sol" => "GPT-5.6 Sol",
        "gpt-5.6-luna" => "GPT-5.6 Luna",
        "gpt-5.6-terra" => "GPT-5.6 Terra",
        "claude-opus-5" => "Claude Opus 5",
        "claude-fable-5" => "Claude Fable 5",
        "claude-sonnet-5" => "Claude Sonnet 5",
        "claude-opus-4-8" => "Claude Opus 4.8",
        "claude-opus-4-6" => "Claude Opus 4.6",
        "claude-sonnet-4-6" => "Claude Sonnet 4.6",
        "claude-opus-4-5" => "Claude Opus 4.5",
        "claude-sonnet-4-5" => "Claude Sonnet 4.5",
        "claude-haiku-4-5" => "Claude Haiku 4.5",
        _ => id,
    }
    .to_owned()
}

pub fn effort_name(value: &str) -> &str {
    match value {
        "" | "default" => "Default",
        "none" => "None",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Maximum",
        "ultra" => "Ultra",
        "ultracode" => "Ultra code",
        _ => value,
    }
}

pub fn canonical_access(value: &str) -> &str {
    match value {
        "readonly" | "read-only" => "read-only",
        "edit" | "workspace-write" => "edit",
        "full" | "danger-full-access" => "full",
        _ => value,
    }
}

pub fn access_name(value: &str) -> &str {
    match canonical_access(value) {
        "read-only" => "Read only",
        "edit" => "Edit files",
        "full" => "Full access",
        "" => "Default access",
        other => other,
    }
}

pub fn provider_name(value: &str) -> &str {
    match value {
        "codex" => "Codex",
        "claude" => "Claude Code",
        "jcode" => "JCode",
        "copilot" => "GitHub Copilot",
        _ => value,
    }
}

pub fn auth_state_name(value: &str) -> &str {
    match value {
        "signed-in" => "Signed in",
        "signed-out" => "Signed out",
        "signing-in" => "Signing in…",
        "failed" => "Sign-in failed",
        "unknown" => "Not checked",
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_win_and_unknown_identifiers_remain_exact() {
        assert_eq!(model_name("gpt-5.3-codex", None), "GPT-5.3 Codex");
        assert_eq!(model_name("claude-haiku-4-5", None), "Claude Haiku 4.5");
        assert_eq!(model_name("default", None), "Automatic");
        assert_eq!(
            model_name("gpt-5.3-codex", Some("Team model")),
            "Team model"
        );
        for id in ["vendor/custom-model:v2", "gpt-team-model", "my-model_7"] {
            assert_eq!(model_name(id, None), id);
        }
    }

    #[test]
    fn legacy_access_aliases_select_the_matching_permission() {
        let wire = "workspace-write";
        assert_eq!(canonical_access(wire), "edit");
        assert_eq!(access_name(wire), "Edit files");
        assert_eq!(access_name("danger-full-access"), "Full access");
        assert_eq!(effort_name("xhigh"), "Extra high");
        assert_eq!(effort_name("ultracode"), "Ultra code");
    }
}
