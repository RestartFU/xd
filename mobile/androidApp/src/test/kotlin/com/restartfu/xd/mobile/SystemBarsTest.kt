package com.restartfu.xd.mobile

import android.graphics.Color
import androidx.core.view.WindowCompat
import com.restartfu.xd.mobile.ui.MinimalThemePreset
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

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
