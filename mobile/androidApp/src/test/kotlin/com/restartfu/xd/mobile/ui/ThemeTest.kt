package com.restartfu.xd.mobile.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeTest {
    @Test fun savedThemeNamesLoadTheirPreset() {
        assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey("dark"))
        assertEquals(MinimalThemePreset.LIGHT, MinimalThemePreset.fromKey("light"))
        assertEquals(MinimalThemePreset.MODERN, MinimalThemePreset.fromKey("modern"))
        assertEquals(MinimalThemePreset.MODERN_LIGHT, MinimalThemePreset.fromKey("modern-light"))
        // Retired and unknown names, like a missing one, load as Dark.
        for (name in listOf("warm", "nord", "dracula", "", null)) {
            assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey(name))
        }
        assertEquals(listOf("dark", "light", "modern", "modern-light"), MinimalThemePreset.entries.map { it.key })
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

    @Test fun onlyTheModernThemesAreGlass() {
        for (theme in MinimalThemePreset.entries) {
            val glass = xdGlass(theme)
            assertEquals(theme.key, theme.key.startsWith("modern"), glass != null)
            if (glass != null) {
                for (fill in listOf(glass.pane, glass.composer, glass.glow)) {
                    assertTrue("$theme pane is not translucent", fill.alpha > 0f && fill.alpha < 1f)
                }
                assertEquals(1f, glass.washFrom.alpha)
                assertEquals(1f, glass.washTo.alpha)
            }
        }
    }

    /** Text sits on the wash in a page, and on panes and the composer over it. */
    @Test fun textReadsOnEveryGlassGround() {
        for (theme in MinimalThemePreset.entries) {
            val glass = xdGlass(theme) ?: continue
            val colors = minimalColors(theme)
            val signals = xdSignals(theme)
            // The glow can reach any part of the wash, so check with and without it.
            for (wash in listOf(
                glass.washFrom,
                glass.washTo,
                glass.glow.compositeOver(glass.washFrom),
                glass.glow.compositeOver(glass.washTo),
            )) {
                for ((ground, color) in listOf(
                    "wash" to wash,
                    "pane" to glass.pane.compositeOver(wash),
                    "composer" to glass.composer.compositeOver(wash),
                )) {
                    assertAtLeast(7.0, colors.onSurface, color, "$theme onSurface on $ground")
                    assertAtLeast(4.5, colors.onSurfaceVariant, color, "$theme onSurfaceVariant on $ground")
                    assertAtLeast(4.5, signals.working, color, "$theme working on $ground")
                    assertAtLeast(4.5, signals.waiting, color, "$theme waiting on $ground")
                    assertAtLeast(4.5, signals.link, color, "$theme link on $ground")
                    assertAtLeast(3.0, signals.faint, color, "$theme faint on $ground")
                }
            }
        }
    }

    private fun assertAtLeast(minimum: Double, foreground: Color, background: Color, what: String) {
        val lighter = maxOf(foreground.luminance(), background.luminance())
        val darker = minOf(foreground.luminance(), background.luminance())
        val ratio = (lighter + 0.05) / (darker + 0.05)
        assertTrue("$what: $ratio < $minimum", ratio >= minimum)
    }
}
