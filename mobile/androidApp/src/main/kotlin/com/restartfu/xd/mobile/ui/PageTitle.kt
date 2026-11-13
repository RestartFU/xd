package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.focusable
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.semantics.heading
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
 */
@Composable
internal fun Modifier.pageHeading(key: Any? = Unit): Modifier {
    val focus = remember { FocusRequester() }
    LaunchedEffect(key) {
        // Not every window can take focus (a page drawn into a capture): then it is left where it is.
        runCatching { focus.requestFocus() }
    }
    return semantics { heading() }.focusRequester(focus).focusable()
}
