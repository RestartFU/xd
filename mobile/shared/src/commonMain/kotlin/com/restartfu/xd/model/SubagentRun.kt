package com.restartfu.xd.model

/** A delegated agent marker captured by the host from a tool call. */
public data class SubagentRun(
    val key: String?,
    val identity: String,
    val status: String,
    val state: SubagentState,
    val detail: String,
    val marker: String,
) {
    public companion object {
        private const val PREFIX = "subagent\n"

        public fun parse(text: String): SubagentRun? {
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

public enum class SubagentState {
    RUNNING,
    SUCCESS,
    FAILURE,
    FINISHED,
}
