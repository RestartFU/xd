package com.restartfu.xd.mobile.ui

import android.app.Activity
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.platform.ViewRootForTest
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/*
 * A soft keyboard for tests. Robolectric has none, so its insets are handed
 * to the Compose view the way the system hands them over when one rises: the
 * keyboard's height at the bottom, with the navigation bar's below it.
 */

/** The Compose view of [activity]'s own content, which reads the window's insets. */
internal fun composeViewOf(activity: Activity): View {
    fun find(view: View): View? = when (view) {
        is ViewRootForTest -> view
        is ViewGroup -> (0 until view.childCount).firstNotNullOfOrNull { find(view.getChildAt(it)) }
        else -> null
    }
    return checkNotNull(find(activity.window.decorView)) { "No Compose view in the activity" }
}

/**
 * Raises a keyboard [keyboardPx] tall over a navigation bar [navigationPx]
 * tall, or lowers it with a [keyboardPx] of 0. The keyboard's inset covers
 * the navigation bar, as it does on a phone.
 */
internal fun showKeyboard(activity: Activity, keyboardPx: Int, navigationPx: Int) {
    val ime = if (keyboardPx > 0) keyboardPx + navigationPx else 0
    val insets = WindowInsetsCompat.Builder()
        .setInsets(WindowInsetsCompat.Type.navigationBars(), Insets.of(0, 0, 0, navigationPx))
        .setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, ime))
        .setVisible(WindowInsetsCompat.Type.navigationBars(), true)
        .setVisible(WindowInsetsCompat.Type.ime(), keyboardPx > 0)
        .build()
    ViewCompat.dispatchApplyWindowInsets(composeViewOf(activity), insets)
}
