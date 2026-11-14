package com.restartfu.xd.model

import kotlin.math.abs
import kotlin.math.roundToLong
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

/** Where one context compaction stands. */
public enum class CompactionState(public val wire: String) {
    RUNNING("running"),
    COMPLETED("completed"),
    FAILED("failed"),
    STOPPED("stopped"),
    ;

    public companion object {
        public fun parse(value: String): CompactionState? = entries.firstOrNull { it.wire == value }
    }
}

/**
 * The row the host keeps for one compaction of a chat's context:
 * `context_compaction\n<state>\n<label>`, rewritten in place as it goes.
 * The label is the host's own sentence, such as "Context compacted 250K → 32K tokens".
 */
public data class ContextCompaction(val state: CompactionState, val label: String) {
    public companion object {
        private const val PREFIX = "context_compaction\n"

        /** The compaction [text] records, or null when it is not one this client understands. */
        public fun parse(text: String): ContextCompaction? {
            if (!text.startsWith(PREFIX)) return null
            val rest = text.removePrefix(PREFIX)
            val state = CompactionState.parse(rest.substringBefore('\n').trim()) ?: return null
            val label = rest.substringAfter('\n', "").trim().takeIf(String::isNotEmpty) ?: return null
            return ContextCompaction(state, label)
        }
    }
}

/** When the host compacts an idle Claude chat: the `idle-compaction-settings` mode. */
public enum class IdleCompactionMode(public val wire: String) {
    /** A send into a chat idle past its prompt cache can compact first. The host's default. */
    ON_RETURN("onReturn"),

    /** The host compacts on its own just before the cache expires. */
    BEFORE_CACHE_EXPIRES("beforeCacheExpires"),
    OFF("off"),
    ;

    public companion object {
        public fun parse(value: String?): IdleCompactionMode? = entries.firstOrNull { it.wire == value }
    }
}

/** The host's idle compaction setting. */
public data class IdleCompactionSettings(val mode: IdleCompactionMode) {
    public companion object {
        /**
         * An `idle-compaction-settings` reply or broadcast, or null when it does
         * not name a mode this client knows, and has to be read again.
         */
        public fun parse(reply: JsonObject): IdleCompactionSettings? =
            IdleCompactionMode.parse((reply["mode"] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull)
                ?.let(::IdleCompactionSettings)
    }
}

/**
 * A chat reply's `compact_before_send`: a plain send into this idle Claude
 * chat can run `/compact` first, from [afterMillis] (Unix ms) onwards, and
 * [tokens] is how much context that would compact.
 */
public data class CompactBeforeSend(val tokens: Long, val afterMillis: Long) {
    /**
     * Whether the composer offers it now: it is due, the host compacts on
     * return, nothing is running or waiting on the reader, and what is typed
     * is not a compaction already.
     */
    public fun offered(
        nowMillis: Long,
        mode: IdleCompactionMode?,
        working: Boolean,
        waitingOnReader: Boolean,
        text: String,
    ): Boolean =
        nowMillis >= afterMillis &&
            mode == IdleCompactionMode.ON_RETURN &&
            !working &&
            !waitingOnReader &&
            !text.trimStart().startsWith(COMMAND, ignoreCase = true)

    public companion object {
        public const val COMMAND: String = "/compact"

        /** The member, or null when it is absent or not a shape this client understands. */
        public fun parse(element: JsonElement?): CompactBeforeSend? {
            val offer = element as? JsonObject ?: return null
            val tokens = (offer["tokens"] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull ?: return null
            val after = (offer["after_ms"] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull ?: return null
            return if (tokens > 0) CompactBeforeSend(tokens, after) else null
        }
    }
}

/**
 * A token count as T3 Code writes it: under 1,000 as it is, then K, M, B or T
 * with no decimals from 100, one from 10 and two below, dropping a fraction
 * that is all zeros: 950, 1K, 1.25K, 1.50K, 12.5K, 251K, 1M. The host's
 * `format_tokens` does the same.
 */
public fun formatTokens(value: Long): String {
    if (value < 1_000) return value.toString()
    val suffixes = listOf("K", "M", "B", "T")
    var scaled = value / 1e3
    var unit = 0
    // The unit is chosen after rounding, so 999,999 reads 1M rather than 1000K.
    while (abs(trimmedNumber(scaled).toDouble()) >= 1e3 && unit < suffixes.lastIndex) {
        scaled /= 1e3
        unit++
    }
    return trimmedNumber(scaled) + suffixes[unit]
}

private fun trimmedNumber(value: Double): String {
    val digits = when {
        abs(value) >= 100 -> 0
        abs(value) >= 10 -> 1
        else -> 2
    }
    var scale = 1L
    repeat(digits) { scale *= 10 }
    val rounded = (value * scale).roundToLong()
    val whole = rounded / scale
    val fraction = rounded % scale
    // Only a fraction of zeros goes: 1.50 stays, 1.00 reads 1.
    if (digits == 0 || fraction == 0L) return whole.toString()
    return "$whole." + fraction.toString().padStart(digits, '0')
}
