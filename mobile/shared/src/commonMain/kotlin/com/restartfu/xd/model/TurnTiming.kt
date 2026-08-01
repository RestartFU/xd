package com.restartfu.xd.model

/**
 * How long a turn has been running, worded the way the desktop words it.
 *
 * Shared rather than written per client: two devices watching the same turn
 * should not disagree about how long it has been going, and the phrasing is
 * the one thing a reader compares between them.
 *
 * Unlike the Crystal original this clamps a negative elapsed time rather than
 * leaving that to the caller. A phone's clock can disagree with the daemon's,
 * and "Working for -3s" is worse than a moment of "0s".
 */
public object TurnTiming {
    public fun format(verb: String, seconds: Long): String {
        val safe = seconds.coerceAtLeast(0)
        return when {
            safe >= 3600 -> "$verb for ${safe / 3600}h ${pad((safe % 3600) / 60)}m"
            safe >= 60 -> "$verb for ${safe / 60}m ${pad(safe % 60)}s"
            else -> "$verb for ${safe}s"
        }
    }

    private fun pad(value: Long): String = value.toString().padStart(2, '0')
}
