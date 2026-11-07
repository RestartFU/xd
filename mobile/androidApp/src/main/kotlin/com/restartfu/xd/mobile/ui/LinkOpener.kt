package com.restartfu.xd.mobile.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
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
    return remember(handler, context) {
        { url ->
            try {
                handler.openUri(url)
            } catch (error: Exception) {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                clipboard?.setPrimaryClip(ClipData.newPlainText("Link", url))
                Toast.makeText(context, "No app could open the link, so it was copied.", Toast.LENGTH_LONG).show()
            }
        }
    }
}
