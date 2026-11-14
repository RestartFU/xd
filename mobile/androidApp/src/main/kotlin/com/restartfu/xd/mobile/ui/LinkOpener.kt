package com.restartfu.xd.mobile.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler

/**
 * Opens a web address, and copies it instead when no app on the phone can
 * open it, so a link never crashes the app and can still be used elsewhere.
 */
@Composable
internal fun rememberLinkOpener(): (String) -> Unit {
    val handler = LocalUriHandler.current
    val context = LocalContext.current
    val notify = LocalNotice.current
    return remember(handler, context, notify) {
        { url ->
            try {
                handler.openUri(url)
            } catch (error: Exception) {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                clipboard?.setPrimaryClip(ClipData.newPlainText("Link", url))
                notify(LINK_COPIED_NOTICE)
            }
        }
    }
}

/** What is said when a link could not be opened and was copied instead. */
internal const val LINK_COPIED_NOTICE = "No app could open the link, so it was copied."

/**
 * Says something brief to the reader where they are looking: the app's
 * snackbar on a page, a line of its own inside a dialog, which is a window the
 * snackbar would sit behind. Never a system toast, which the app cannot place,
 * style or have read out with the rest of the screen.
 */
internal val LocalNotice = staticCompositionLocalOf<(String) -> Unit> { {} }

/**
 * Says that something was removed, on the app's snackbar, with an Undo: true
 * once Undo is pressed, false once the notice has gone without it. Null where
 * there is no snackbar to say it on.
 */
internal val LocalUndoNotice = staticCompositionLocalOf<(suspend (String) -> Boolean)?> { null }
