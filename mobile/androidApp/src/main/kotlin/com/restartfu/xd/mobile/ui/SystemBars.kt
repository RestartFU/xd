package com.restartfu.xd.mobile.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import androidx.core.view.WindowCompat

/**
 * Gives a full-screen dialog's window the theme's system bars. A dialog is a
 * window of its own, so what the activity set for its bars does not reach it:
 * without this a page drawn in Light sits under light icons it hides.
 * Call it from inside the dialog's content.
 */
@Composable
internal fun DialogSystemBars() {
    val light = LocalXdTheme.current.isLight
    val view = LocalView.current
    val window = (view.parent as? DialogWindowProvider)?.window ?: return
    SideEffect {
        // Clear bars, as the activity has: the page's own surface shows behind them.
        @Suppress("DEPRECATION")
        window.statusBarColor = Color.Transparent.toArgb()
        @Suppress("DEPRECATION")
        window.navigationBarColor = Color.Transparent.toArgb()
        WindowCompat.getInsetsController(window, view).apply {
            isAppearanceLightStatusBars = light
            isAppearanceLightNavigationBars = light
        }
    }
}
