package com.restartfu.xd.net

internal class Backoff {
    private var attempt: Int = 0

    fun nextDelayMillis(): Long {
        val delay = when (attempt++) {
            0 -> 2_000L
            1 -> 5_000L
            2 -> 15_000L
            3 -> 60_000L
            else -> 120_000L
        }
        return delay
    }

    fun reset() {
        attempt = 0
    }
}
