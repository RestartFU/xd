package com.restartfu.xd.mobile.ui

import android.os.Build
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
        // Android 8.0 cannot draw dark navigation buttons: on Light they get
        // the activity's dark scrim behind them instead of the light page.
        @Suppress("DEPRECATION")
        window.navigationBarColor = navigationBarColor(light, Build.VERSION.SDK_INT).toArgb()
        WindowCompat.getInsetsController(window, view).apply {
            isAppearanceLightStatusBars = light
            isAppearanceLightNavigationBars = light
        }
    }
}

/** A full-screen dialog's navigation bar: clear, except behind light buttons on Android 8.0. */
internal fun navigationBarColor(light: Boolean, sdk: Int): Color =
    if (light && sdk < Build.VERSION_CODES.O_MR1) NavigationBarDarkScrim else Color.Transparent
