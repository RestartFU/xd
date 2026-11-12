package com.restartfu.xd.mobile.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeTest {
    @Test fun savedThemeNamesLoadTheirPreset() {
        assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey("dark"))
        assertEquals(MinimalThemePreset.LIGHT, MinimalThemePreset.fromKey("light"))
        assertEquals(MinimalThemePreset.MODERN, MinimalThemePreset.fromKey("modern"))
        // Retired and unknown names, like a missing one, load as Dark.
        for (name in listOf("warm", "nord", "dracula", "", null)) {
            assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey(name))
        }
        assertEquals(listOf("dark", "light", "modern"), MinimalThemePreset.entries.map { it.key })
    }

    @Test fun everyThemeKeepsTextAndSignalsReadable() {
        for (theme in MinimalThemePreset.entries) {
            val colors = minimalColors(theme)
            val signals = xdSignals(theme)
            for ((role, ground) in listOf(
                "background" to colors.background,
                "surface" to colors.surface,
                "surfaceVariant" to colors.surfaceVariant,
                "surfaceContainerLow" to colors.surfaceContainerLow,
                "surfaceContainer" to colors.surfaceContainer,
                "surfaceContainerHigh" to colors.surfaceContainerHigh,
            )) {
                assertAtLeast(7.0, colors.onSurface, ground, "$theme onSurface on $role")
                assertAtLeast(4.5, colors.onSurfaceVariant, ground, "$theme onSurfaceVariant on $role")
                assertAtLeast(4.5, signals.working, ground, "$theme working on $role")
                assertAtLeast(4.5, signals.waiting, ground, "$theme waiting on $role")
                assertAtLeast(4.5, signals.link, ground, "$theme link on $role")
                assertAtLeast(3.0, signals.faint, ground, "$theme faint on $role")
            }
            assertAtLeast(4.5, colors.onPrimary, colors.primary, "$theme onPrimary")
            assertAtLeast(4.5, colors.error, colors.surface, "$theme error")
        }
    }

    private fun assertAtLeast(minimum: Double, foreground: Color, background: Color, what: String) {
        val lighter = maxOf(foreground.luminance(), background.luminance())
        val darker = minOf(foreground.luminance(), background.luminance())
        val ratio = (lighter + 0.05) / (darker + 0.05)
        assertTrue("$what: $ratio < $minimum", ratio >= minimum)
    }
}
