package com.restartfu.xd.model

import com.restartfu.xd.protocol.BackendReply

/** Presentation labels only. Request payloads keep their original identifiers. */
public object DisplayLabels {
    public fun model(
        id: String?,
        backend: String = "",
        catalog: List<BackendReply> = emptyList(),
    ): String {
        if (id.isNullOrBlank()) return "Default model"
        return catalog.firstOrNull { it.id == backend }
            ?.models?.firstOrNull { it.id == id }?.name?.takeIf { it.isNotBlank() }
            ?: MODEL_NAMES[id]
            ?: id
    }

    public fun backend(id: String?, catalog: List<BackendReply> = emptyList()): String {
        if (id.isNullOrBlank()) return "Agent"
        return catalog.firstOrNull { it.id == id }?.name?.takeIf { it.isNotBlank() }
            ?: when (id) {
                "codex" -> "Codex"
                "claude" -> "Claude Code"
                "jcode" -> "JCode"
                "copilot" -> "GitHub Copilot"
                else -> id
            }
    }

    public fun effort(value: String?): String = when (value) {
        null, "" -> "Default"
        "low" -> "Low"
        "medium" -> "Medium"
        "high" -> "High"
        "xhigh" -> "Extra high"
        "max" -> "Maximum"
        "ultra" -> "Ultra"
        "ultracode" -> "Ultra code"
        else -> if (value.isBlank()) "Default" else value
    }

    /** Legacy access names select the same control without changing the wire value. */
    public fun canonicalAccess(value: String?): String? = when (value) {
        "readonly", "read-only" -> "read-only"
        "edit", "workspace-write" -> "edit"
        "full", "danger-full-access" -> "full"
        else -> value
    }

    public fun access(value: String?): String = when (canonicalAccess(value)) {
        null, "" -> "Default access"
        "read-only" -> "Read only"
        "edit" -> "Edit files"
        "full" -> "Full access"
        else -> if (value.isNullOrBlank()) "Default access" else value
    }

    private val MODEL_NAMES = mapOf(
        "default" to "Automatic",
        "gpt-5" to "GPT-5",
        "gpt-5-mini" to "GPT-5 Mini",
        "gpt-5-nano" to "GPT-5 Nano",
        "gpt-5-codex" to "GPT-5 Codex",
        "gpt-5.1" to "GPT-5.1",
        "gpt-5.1-codex" to "GPT-5.1 Codex",
        "gpt-5.1-codex-mini" to "GPT-5.1 Codex Mini",
        "gpt-5.1-codex-max" to "GPT-5.1 Codex Max",
        "gpt-5.2" to "GPT-5.2",
        "gpt-5.2-codex" to "GPT-5.2 Codex",
        "gpt-5.3-codex" to "GPT-5.3 Codex",
        "gpt-5.3-codex-spark" to "GPT-5.3 Codex Spark",
        "gpt-5.4" to "GPT-5.4",
        "gpt-5.4-mini" to "GPT-5.4 Mini",
        "gpt-5.4-nano" to "GPT-5.4 Nano",
        "gpt-5.5" to "GPT-5.5",
        "gpt-5.6-sol" to "GPT-5.6 Sol",
        "gpt-5.6-luna" to "GPT-5.6 Luna",
        "gpt-5.6-terra" to "GPT-5.6 Terra",
        "claude-opus-5" to "Claude Opus 5",
        "claude-fable-5" to "Claude Fable 5",
        "claude-sonnet-5" to "Claude Sonnet 5",
        "claude-sonnet-4-5" to "Claude Sonnet 4.5",
        "claude-sonnet-4-6" to "Claude Sonnet 4.6",
        "claude-haiku-4-5" to "Claude Haiku 4.5",
        "claude-opus-4-8" to "Claude Opus 4.8",
    )
}
