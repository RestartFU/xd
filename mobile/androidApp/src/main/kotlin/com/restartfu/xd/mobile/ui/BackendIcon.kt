package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R

/**
 * The assistant mark for a chat, matching the desktop sidebar.
 *
 * Backend ids come from the daemon's catalog. An unknown one draws nothing
 * rather than a placeholder: a new backend should not make old clients show a
 * broken glyph.
 */
@Composable
internal fun BackendIcon(
    backend: String,
    modifier: Modifier = Modifier,
    size: Dp = 18.dp,
) {
    val drawable = when (backend) {
        "claude" -> R.drawable.ic_backend_claude
        "codex" -> R.drawable.ic_backend_codex
        else -> return
    }

    // Claude keeps its brand orange; the OpenAI mark is symbolic on the
    // desktop, so here it takes the ambient content colour and follows the
    // light and dark themes.
    val tint = if (backend == "claude") Color.Unspecified else LocalContentColor.current

    Icon(
        painter = painterResource(drawable),
        contentDescription = backend,
        modifier = modifier.size(size),
        tint = tint,
    )
}
