package com.restartfu.xd.model

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

/** How a CI watch, or one of its checks, stands. */
public enum class CiState { RUNNING, PASSED, FAILED, STOPPED, SKIPPED }

/** One check on a pull request's head, as its CI card lists it. */
public data class CiCheck(
    /** The workflow and check, such as `pr / linux`. */
    val name: String,
    val state: CiState,
    /** What the check says, such as Running, Passed or Timed out. */
    val label: String,
)

/**
 * The card the host keeps for a CI watch an agent asked for with xd's
 * watch_ci tool: `ci_run\n` and a JSON record the host rewrites in place as
 * the pull request's checks go. See docs/protocol.md.
 */
public data class CiRun(
    val url: String,
    val number: Long?,
    val title: String,
    val head: String,
    val state: CiState,
    val checks: List<CiCheck>,
    /** Why the watch stopped, or what it waits for. */
    val note: String?,
    val startedAt: Long?,
    val finishedAt: Long?,
) {
    /** The pull request's checks page, which the card opens. */
    val checksUrl: String get() = "$url/checks"

    /** The card's one-line summary of the checks. */
    val summary: String
        get() {
            note?.let { return it }
            if (checks.isEmpty()) return "Waiting for checks to start."
            val running = checks.count { it.state == CiState.RUNNING }
            val failed = checks.count { it.state == CiState.FAILED }
            val passed = checks.count { it.state == CiState.PASSED }
            return when {
                running > 0 -> "$running of ${checks.size} checks running"
                failed > 0 -> "$failed failed, $passed passed"
                else -> "$passed passed"
            }
        }

    public companion object {
        private const val PREFIX = "ci_run\n"

        public fun parse(text: String): CiRun? {
            if (!text.startsWith(PREFIX)) return null
            val record = runCatching {
                Json.parseToJsonElement(text.removePrefix(PREFIX)) as? JsonObject
            }.getOrNull() ?: return null
            fun field(json: JsonObject, name: String) =
                (json[name] as? JsonPrimitive)?.contentOrNull.orEmpty()
            val url = field(record, "url").takeIf { it.startsWith("https://") } ?: return null
            val checks = (record["checks"] as? JsonArray).orEmpty().mapNotNull { entry ->
                val check = entry as? JsonObject ?: return@mapNotNull null
                val name = field(check, "name").ifBlank { return@mapNotNull null }
                val workflow = field(check, "workflow")
                val (state, label) = when (field(check, "state")) {
                    "passed" -> CiState.PASSED to "Passed"
                    "skipped" -> CiState.SKIPPED to "Skipped"
                    "failed" -> CiState.FAILED to
                        field(check, "conclusion").ifBlank { "failed" }.replaceFirstChar { it.uppercase() }
                    else -> CiState.RUNNING to "Running"
                }
                CiCheck(if (workflow.isBlank()) name else "$workflow / $name", state, label)
            }
            val waiting = field(record, "waiting_for")
            return CiRun(
                url = url,
                number = (record["number"] as? JsonPrimitive)?.longOrNull,
                title = field(record, "title"),
                head = field(record, "head"),
                state = when (field(record, "status")) {
                    "passed" -> CiState.PASSED
                    "failed" -> CiState.FAILED
                    "stopped" -> CiState.STOPPED
                    else -> CiState.RUNNING
                },
                checks = checks,
                note = field(record, "reason").ifBlank { null }
                    ?: waiting.takeIf(String::isNotBlank)?.let { "Waiting for GitHub to show $it." },
                startedAt = (record["started_at"] as? JsonPrimitive)?.longOrNull,
                finishedAt = (record["finished_at"] as? JsonPrimitive)?.longOrNull,
            )
        }
    }
}
