package com.restartfu.xd.model

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

/**
 * A delegated agent. The host keeps one live record per agent and rewrites it
 * as the agent works, so this is always the newest snapshot: who it is, what it
 * is doing, the steps it took, and its report once it finishes.
 */
public data class SubagentRun(
    val key: String?,
    val identity: String,
    val status: String,
    val state: SubagentState,
    val detail: String,
    val marker: String,
    val provider: String = "",
    val model: String = "",
    val prompt: String = "",
    val background: Boolean = false,
    /** What the agent is doing right now, while it runs. */
    val activity: String = "",
    val steps: List<SubagentStep> = emptyList(),
    val toolUses: Long = 0,
    val tokens: Long? = null,
    val durationMillis: Long? = null,
    /** Its newest message while it works. */
    val latest: String = "",
    /** Its final report. */
    val summary: String = "",
) {
    /** "2 tools · 13.9k tokens · 2.9s", whatever of it is known. */
    val stats: String
        get() = listOfNotNull(
            toolUses.takeIf { it > 0 }?.let { "$it tool${if (it == 1L) "" else "s"}" },
            tokens?.let { "${compactCount(it)} tokens" },
            durationMillis?.let(::compactDuration),
        ).joinToString(" · ")

    public companion object {
        private const val PREFIX = "subagent\n"
        private const val RECORD_PREFIX = "subagent_run\n"

        public fun parse(text: String): SubagentRun? =
            if (text.startsWith(RECORD_PREFIX)) parseRecord(text) else parseLegacy(text)

        private fun parseRecord(text: String): SubagentRun? {
            val record = runCatching {
                Json.parseToJsonElement(text.removePrefix(RECORD_PREFIX)) as? JsonObject
            }.getOrNull() ?: return null
            fun field(name: String) = (record[name] as? JsonPrimitive)?.contentOrNull.orEmpty()
            val state = when (field("status")) {
                "completed" -> SubagentState.SUCCESS
                "failed" -> SubagentState.FAILURE
                "stopped" -> SubagentState.FINISHED
                else -> SubagentState.RUNNING
            }
            return SubagentRun(
                key = field("key").ifBlank { null },
                identity = field("agent").ifBlank { "Agent" },
                status = when (state) {
                    SubagentState.RUNNING -> "Running"
                    SubagentState.SUCCESS -> "Done"
                    SubagentState.FAILURE -> "Failed"
                    SubagentState.FINISHED -> "Stopped"
                },
                state = state,
                detail = field("description"),
                marker = text,
                provider = field("provider"),
                model = field("model"),
                prompt = field("prompt"),
                background = (record["background"] as? JsonPrimitive)?.booleanOrNull ?: false,
                activity = field("activity"),
                steps = (record["steps"] as? JsonArray).orEmpty().mapNotNull { step ->
                    val fields = step as? JsonObject ?: return@mapNotNull null
                    SubagentStep(
                        tool = (fields["tool"] as? JsonPrimitive)?.contentOrNull.orEmpty(),
                        text = (fields["text"] as? JsonPrimitive)?.contentOrNull.orEmpty(),
                    )
                },
                toolUses = (record["tool_uses"] as? JsonPrimitive)?.longOrNull ?: 0,
                tokens = (record["tokens"] as? JsonPrimitive)?.longOrNull,
                durationMillis = (record["duration_ms"] as? JsonPrimitive)?.longOrNull,
                latest = field("latest"),
                summary = field("summary"),
            )
        }

        private fun parseLegacy(text: String): SubagentRun? {
            if (!text.startsWith(PREFIX)) return null
            val lines = text.removePrefix(PREFIX).split('\n')
            val (key, identity, task) = when (lines.size) {
                2 -> Triple(null, lines[0], lines[1])
                3 -> Triple(lines[0].ifBlank { null }, lines[1], lines[2])
                else -> return null
            }
            if (identity.isBlank() || task.isBlank()) return null

            val parts = task.split(" · ")
            val reported = parts.first().trim()
            val state = when (reported.lowercase()) {
                "completed", "done", "succeeded" -> SubagentState.SUCCESS
                "failed", "errored", "spawn failed" -> SubagentState.FAILURE
                "interrupted", "stopped", "not found" -> SubagentState.FINISHED
                else -> SubagentState.RUNNING
            }
            val hasReportedState = state != SubagentState.RUNNING ||
                reported.equals("running", ignoreCase = true)
            val detail = if (hasReportedState) {
                parts.drop(1).joinToString(" · ").trim()
            } else {
                task.trim()
            }
            val status = when (state) {
                SubagentState.RUNNING -> "Running"
                SubagentState.SUCCESS -> "Completed"
                SubagentState.FAILURE -> "Failed"
                SubagentState.FINISHED -> reported
            }
            return SubagentRun(
                key = key,
                identity = identity.trim(),
                status = status,
                state = state,
                detail = detail,
                marker = text,
            )
        }
    }
}

/** One of a subagent's own tool calls, as a single line. */
public data class SubagentStep(
    val tool: String,
    val text: String,
)

public enum class SubagentState {
    RUNNING,
    SUCCESS,
    FAILURE,
    FINISHED,
}

/** 950, 13.9k, 1.2M. */
public fun compactCount(value: Long): String = when {
    value < 1_000 -> value.toString()
    value < 1_000_000 -> trimDecimal(value / 1_000.0, "k")
    else -> trimDecimal(value / 1_000_000.0, "M")
}

private fun trimDecimal(value: Double, unit: String): String =
    if (value >= 100) {
        "${kotlin.math.round(value).toLong()}$unit"
    } else {
        val tenths = kotlin.math.round(value * 10).toLong()
        if (tenths % 10 == 0L) "${tenths / 10}$unit" else "${tenths / 10}.${tenths % 10}$unit"
    }

/** 2.9s, 47s, 3m 05s, 1h 02m. */
public fun compactDuration(millis: Long): String {
    val seconds = millis / 1_000
    if (seconds < 10) {
        val tenths = kotlin.math.round(millis / 100.0).toLong()
        return "${tenths / 10}.${tenths % 10}s"
    }
    if (seconds < 60) return "${seconds}s"
    val minutes = seconds / 60
    if (minutes < 60) return "${minutes}m ${(seconds % 60).toString().padStart(2, '0')}s"
    return "${minutes / 60}h ${(minutes % 60).toString().padStart(2, '0')}m"
}
