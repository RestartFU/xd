package com.restartfu.xd.model

import com.restartfu.xd.net.Link

/**
 * What the one line under a page's header says while the host is not
 * connected. There is none while it is: a working link needs no words.
 */
public data class ConnectionNotice(
    /** What is happening, and when the next attempt is. */
    val title: String,
    /** Why the link is down, as the transport said it; null when it gave no reason. */
    val reason: String?,
    /** A link that failed, told in the error role. A first connection is told quietly. */
    val failure: Boolean,
    /** Waiting before the next attempt, which can be made at once instead. */
    val waiting: Boolean = false,
) {
    /** The line as drawn. */
    val text: String get() = if (reason == null) title else "$title · $reason"

    /**
     * What a screen reader is told: the same without the countdown, so a
     * second passing is not a change worth announcing.
     */
    val spoken: String
        get() = when {
            !failure -> "Connecting to the host"
            reason == null -> "Not connected to the host. Reconnecting."
            else -> "Not connected to the host: $reason. Reconnecting."
        }
}

/**
 * The notice for [link]. [lastFailure] is why the link last went down since
 * it was last up, so the attempts between waits keep saying it; [secondsLeft]
 * counts down to the next attempt while it waits.
 */
public fun connectionNotice(link: Link, lastFailure: String?, secondsLeft: Long? = null): ConnectionNotice? =
    when (link) {
        is Link.Up, is Link.Fatal -> null
        is Link.Down -> ConnectionNotice(
            title = secondsLeft?.takeIf { it > 0 }?.let { "Reconnecting in ${countdown(it)}" } ?: "Reconnecting…",
            reason = reason(link.message),
            failure = true,
            waiting = true,
        )
        Link.Idle, Link.Connecting ->
            if (lastFailure == null) {
                ConnectionNotice("Connecting…", null, failure = false)
            } else {
                ConnectionNotice("Reconnecting…", reason(lastFailure), failure = true)
            }
    }

/** Whole seconds until a wait of [millis] ends, rounded up so it never shows 0 while waiting. */
public fun secondsUntil(millis: Long): Long = if (millis <= 0) 0 else (millis + 999) / 1000

private fun countdown(seconds: Long): String =
    if (seconds < 60) "$seconds s" else "${(seconds + 59) / 60} min"

/** One line of the transport's message, without the full stop a sentence would repeat. */
private fun reason(message: String): String? =
    message.lineSequence().map(String::trim).firstOrNull(String::isNotEmpty)?.trimEnd('.')?.takeIf(String::isNotEmpty)
