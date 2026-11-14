package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.focusable
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics

/*
 * Where a page change is told. Each page's root carries its title as a pane
 * title, so a screen reader says which page appeared; the page's heading
 * takes the focus as it appears, so a keyboard, a switch or a screen reader
 * goes on from the top of the new page, not from wherever the old one left it.
 */

/** The root of a page called [title]: a screen reader announces it as the page appears or its title changes. */
internal fun Modifier.page(title: String): Modifier = semantics { paneTitle = title }

/**
 * A page's heading, which takes the focus when it first appears, and again
 * whenever [key] changes (a page whose heading stays while its content changes).
 *
 * On a list page [returnTo] says which of its rows was opened: when Back
 * returns to the list, that row takes the focus instead, so the reader goes
 * on from where they were. The heading takes it when the row is gone.
 */
@Composable
internal fun Modifier.pageHeading(key: Any? = Unit, returnTo: ReturnFocus? = null): Modifier {
    val focus = remember { FocusRequester() }
    if (returnTo != null) {
        DisposableEffect(returnTo, focus) {
            returnTo.heading = focus
            onDispose { if (returnTo.heading === focus) returnTo.heading = null }
        }
    }
    LaunchedEffect(key) {
        if (returnTo?.restore() == true) return@LaunchedEffect
        // Not every window can take focus (a page drawn into a capture): then it is left where it is.
        runCatching { focus.requestFocus() }
    }
    return semantics { heading() }.focusRequester(focus).focusable()
}

/**
 * Which row of a list page was opened, so that Back to the list puts the
 * focus on that row again rather than on the top of the page. Rows say
 * which they are with [returnsFocus]; the page's [pageHeading] puts the focus
 * back as the list appears, and a list whose heading stays on screen calls
 * [restore] itself.
 */
@Stable
internal class ReturnFocus(opened: String? = null) {
    /** The row opened last, until the focus has gone back to it. */
    var opened: String? by mutableStateOf(opened)
        private set

    internal val rows = mutableMapOf<String, FocusRequester>()
    internal var heading: FocusRequester? = null

    /** The row called [key] is being opened. */
    fun open(key: String) {
        opened = key
    }

    /**
     * Puts the focus on the row that was opened, or on the page's heading
     * when that row is no longer on screen. Nothing happens, and false is
     * answered, when no row was opened since the last time.
     */
    suspend fun restore(): Boolean {
        val key = opened ?: return false
        // One frame, so the rows of a lazy list have been laid out and said which they are.
        withFrameNanos { }
        opened = null
        val target = rows[key] ?: heading ?: return true
        runCatching { target.requestFocus() }
        return true
    }

    companion object {
        val Saver: Saver<ReturnFocus, String> = Saver(save = { it.opened }, restore = { ReturnFocus(it) })
    }
}

/** The list page's [ReturnFocus], kept through a rotation. */
@Composable
internal fun rememberReturnFocus(): ReturnFocus = rememberSaveable(saver = ReturnFocus.Saver) { ReturnFocus() }

/** The [ReturnFocus] of the list page around; null outside one. */
internal val LocalReturnFocus = staticCompositionLocalOf<ReturnFocus?> { null }

/**
 * A row of a list page called [key], put before its `clickable`: when Back
 * returns to the list after it was opened, it takes the focus.
 *
 * A clickable row takes the focus only from a keyboard; while the screen is
 * touched, as it is with a screen reader, this takes it in the row's place.
 * Never both, so a keyboard still stops on the row once.
 */
@Composable
internal fun Modifier.returnsFocus(key: String, returns: ReturnFocus? = LocalReturnFocus.current): Modifier {
    if (returns == null) return this
    val focus = remember { FocusRequester() }
    DisposableEffect(returns, key, focus) {
        returns.rows[key] = focus
        onDispose { if (returns.rows[key] === focus) returns.rows.remove(key) }
    }
    val inputs = LocalInputModeManager.current
    return focusRequester(focus)
        .focusProperties { canFocus = inputs.inputMode == InputMode.Touch }
        .focusable()
}

/**
 * Text that appears or changes on its own, such as an error after a tap or a
 * notice from the host: a screen reader says it when it does, after whatever
 * it is reading, without moving the focus.
 */
internal fun Modifier.announced(): Modifier = semantics { liveRegion = LiveRegionMode.Polite }
