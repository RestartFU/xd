package com.restartfu.xd.model

import kotlin.math.roundToLong
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull

/**
 * The quota that applies to a chat: a share of the usage limit its account
 * works under, in percentage points of that limit.
 */
public data class ChatQuota(
    /** The quota, a whole number from 1 to 100. */
    val percent: Int,
    /** How much of the limit the chat has used since its count began. */
    val used: Double = 0.0,
    /** The chat has its own quota rather than the host's default. */
    val own: Boolean = false,
) {
    /** [used] as shown: a whole number of percentage points. */
    val usedLabel: String get() = wholePercent(used)

    public companion object {
        public const val MIN_PERCENT: Int = 1
        public const val MAX_PERCENT: Int = 100

        /** What a client offers when turning the host's default on. */
        public const val DEFAULT_PERCENT: Int = 10

        /**
         * The `quota` member of a `chat` reply, or null when there is none or
         * it is not one this client understands: no quota is then shown.
         */
        public fun parse(element: JsonElement?): ChatQuota? {
            val quota = element as? JsonObject ?: return null
            val percent = (quota["percent"] as? JsonPrimitive)?.wholePercentOrNull() ?: return null
            val used = (quota["used"] as? JsonPrimitive)?.takeUnless { it.isString }?.doubleOrNull
                ?.takeIf { it.isFinite() && it >= 0 } ?: 0.0
            val own = (quota["own"] as? JsonPrimitive)?.takeUnless { it.isString }?.booleanOrNull ?: false
            return ChatQuota(percent, used, own)
        }

        /**
         * A `percent` member: a whole number from 1 to 100, or null for
         * anything else, including the host's `null` for no limit.
         */
        public fun parsePercent(element: JsonElement?): Int? =
            (element as? JsonPrimitive)?.wholePercentOrNull()

        /** The quota offered after a chat used [last]: twice as much, up to the whole limit. */
        public fun nextPercent(last: Int?): Int =
            ((last ?: DEFAULT_PERCENT / 2) * 2).coerceIn(MIN_PERCENT, MAX_PERCENT)

        /** What was typed as a quota, or null unless it is a whole number from 1 to 100. */
        public fun typedPercent(text: String): Int? =
            text.trim().removeSuffix("%").trim().toIntOrNull()?.takeIf { it in MIN_PERCENT..MAX_PERCENT }

        private fun JsonPrimitive.wholePercentOrNull(): Int? {
            if (isString) return null
            val value = doubleOrNull ?: return null
            if (!value.isFinite() || value != kotlin.math.floor(value)) return null
            return value.toInt().takeIf { it in MIN_PERCENT..MAX_PERCENT }
        }

        private fun wholePercent(value: Double): String = value.roundToLong().toString()
    }
}

/**
 * The host's default chat quota: the [ChatQuota.percent] every chat without
 * its own quota gets, or null while no chat is limited.
 */
public data class ChatQuotaSettings(val percent: Int?) {
    public companion object {
        /**
         * A `chat-quota-settings` reply or broadcast, or null when it does not
         * say what the default is, and has to be read again.
         */
        public fun parse(reply: JsonObject): ChatQuotaSettings? =
            if ("percent" in reply) ChatQuotaSettings(ChatQuota.parsePercent(reply["percent"])) else null
    }
}
