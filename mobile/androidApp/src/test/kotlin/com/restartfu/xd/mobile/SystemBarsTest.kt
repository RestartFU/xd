package com.restartfu.xd.mobile

import android.app.UiModeManager
import android.graphics.Color
import androidx.core.view.WindowCompat
import com.restartfu.xd.mobile.ui.MinimalThemePreset
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowLooper

/**
 * The real activity draws behind the system bars and gives them icons that
 * read on the saved theme, whatever the system's own night mode says.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class SystemBarsTest {
    // By day the launch window's own theme asks for dark icons, at night for
    // light ones; each of the app's themes is checked against the other.
    @Test fun darkGetsLightIconsWhileTheSystemIsInDayMode() = barsFollow(MinimalThemePreset.DARK)

    @Test @Config(qualifiers = "night")
    fun lightGetsDarkIconsWhileTheSystemIsInNightMode() = barsFollow(MinimalThemePreset.LIGHT)

    @Test fun lightGetsDarkIconsByDay() = barsFollow(MinimalThemePreset.LIGHT)

    @Test @Config(qualifiers = "night")
    fun darkGetsLightIconsAtNight() = barsFollow(MinimalThemePreset.DARK)

    // The launch window wears the saved theme's ground: Android is told the
    // app's night mode on start, and again whenever the theme changes.
    @Test fun theLaunchWindowFollowsTheSavedThemeNotTheSystem() {
        val application = RuntimeEnvironment.getApplication() as XdApplication
        application.settings.setTheme(MinimalThemePreset.LIGHT)
        Robolectric.buildActivity(MainActivity::class.java).create()
        ShadowLooper.idleMainLooper()
        val modes = application.getSystemService(UiModeManager::class.java)
        assertEquals(UiModeManager.MODE_NIGHT_NO, shadowOf(modes).applicationNightMode)
        application.settings.setTheme(MinimalThemePreset.DARK)
        ShadowLooper.idleMainLooper()
        assertEquals(UiModeManager.MODE_NIGHT_YES, shadowOf(modes).applicationNightMode)
    }

    private fun barsFollow(theme: MinimalThemePreset) {
        val application = RuntimeEnvironment.getApplication() as XdApplication
        application.settings.setTheme(theme)
        val activity = Robolectric.buildActivity(MainActivity::class.java).create().get()
        val window = activity.window
        val bars = WindowCompat.getInsetsController(window, window.decorView)
        assertEquals(theme.isLight, bars.isAppearanceLightStatusBars)
        assertEquals(theme.isLight, bars.isAppearanceLightNavigationBars)
        @Suppress("DEPRECATION")
        assertEquals(Color.TRANSPARENT, window.statusBarColor)
        @Suppress("DEPRECATION")
        assertEquals(Color.TRANSPARENT, window.navigationBarColor)
    }
}
