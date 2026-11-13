package com.restartfu.xd.mobile.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThemeTest {
    @Test fun cornersComeFromTheThemesShapes() {
        // Material's own scale, so its components keep their shapes.
        assertEquals(androidx.compose.material3.Shapes().small, XdShapes.small)
        assertEquals(androidx.compose.material3.Shapes().extraLarge, XdShapes.extraLarge)
        // No radius is written out where a surface is drawn: only here and in Theme.kt.
        val sources = java.io.File("src/main/kotlin").walkTopDown().filter { it.extension == "kt" }.toList()
        assertTrue("run from the androidApp module", sources.size > 20)
        val literal = sources.filter { it.name != "Theme.kt" && "RoundedCornerShape(" in it.readText() }
        assertEquals("corner radii outside ui/Theme.kt", emptyList<String>(), literal.map { it.name })
    }

    @Test fun savedThemeNamesLoadTheirPreset() {
        assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey("dark"))
        assertEquals(MinimalThemePreset.LIGHT, MinimalThemePreset.fromKey("light"))
        // Retired and unknown names, like a missing one, load as Dark.
        for (name in listOf("warm", "nord", "dracula", "", null)) {
            assertEquals(MinimalThemePreset.DARK, MinimalThemePreset.fromKey(name))
        }
        assertEquals(listOf("dark", "light"), MinimalThemePreset.entries.map { it.key })
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

    @Test fun everyThemeIsTranslucentGlassOverAnOpaqueBackdrop() {
        for (theme in MinimalThemePreset.entries) {
            val glass = xdGlass(theme)
            for (fill in listOf(glass.pane, glass.composer)) {
                assertTrue("$theme pane is not translucent", fill.alpha > 0f && fill.alpha < 1f)
            }
            // Overlays sit over scrolling content, so they must not let it through.
            assertEquals("$theme overlay", 1f, glass.overlay().alpha)
            assertEquals(1f, glass.backdrop.alpha)
        }
    }

    /** Text sits on the backdrop in a page, and on panes and the composer over it. */
    @Test fun textReadsOnEveryGlassGround() {
        for (theme in MinimalThemePreset.entries) {
            val glass = xdGlass(theme)
            val colors = minimalColors(theme)
            val signals = xdSignals(theme)
            for ((ground, color) in listOf(
                "backdrop" to glass.backdrop,
                "pane" to glass.pane.compositeOver(glass.backdrop),
                "composer" to glass.composer.compositeOver(glass.backdrop),
                "overlay" to glass.overlay(),
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

    /** The desktop's sixteen ANSI colours for each theme, colour for colour. */
    @Test fun theTerminalPaletteIsTheDesktopsForEachTheme() {
        val expected = mapOf(
            MinimalThemePreset.DARK to "1f2329 e06c75 98c379 e5c07b 61afef c678dd 56b6c2 d7dae0 " +
                "5c6370 ff7a85 b3e180 ffd68a 7dbaff d99bff 75d5e0 ffffff",
            MinimalThemePreset.LIGHT to "1b1e2c b3202f 1a6b3c 7a5200 2a52be 8a2da5 0e6471 4b5165 " +
                "5a6074 c4283a 1f7440 855600 3558d6 9c36b5 0b6f80 1b1e2c",
        )
        for ((theme, colours) in expected) {
            assertEquals(
                "$theme",
                colours.split(' ').map { Color(0xFF000000 or it.toLong(16)) },
                terminalPalette(theme),
            )
        }
    }

    /** Light's terminal colours all read on the ground the terminal is drawn on. */
    @Test fun lightTerminalColoursReadOnTheTerminalGround() {
        val ground = minimalColors(MinimalThemePreset.LIGHT).surface
        terminalPalette(MinimalThemePreset.LIGHT).forEachIndexed { index, colour ->
            assertAtLeast(4.5, colour, ground, "Light terminal colour $index")
        }
        // Dark's own colours, bar its black, read on its ground too.
        val dark = minimalColors(MinimalThemePreset.DARK).surface
        terminalPalette(MinimalThemePreset.DARK).forEachIndexed { index, colour ->
            if (index != 0 && index != 8) assertAtLeast(4.5, colour, dark, "Dark terminal colour $index")
        }
    }

    /** A status mark is a shape to be made out, on a page or on a pane over it. */
    @Test fun statusMarksStandOutFromEveryGround() {
        for (theme in MinimalThemePreset.entries) {
            val glass = xdGlass(theme)
            val signals = xdSignals(theme)
            for ((ground, color) in listOf(
                "backdrop" to glass.backdrop,
                "pane" to glass.pane.compositeOver(glass.backdrop),
                "overlay" to glass.overlay(),
                "menu" to minimalColors(theme).surfaceContainer,
            )) {
                for ((mark, colour) in listOf(
                    "working" to signals.working, "waiting" to signals.waiting,
                    "done" to signals.done, "paused" to signals.faint,
                )) {
                    assertAtLeast(3.0, colour, color, "$theme $mark mark on $ground")
                }
            }
        }
    }

    /** The connection line's failure band keeps its text readable. */
    @Test fun theConnectionLineReadsInBothThemes() {
        for (theme in MinimalThemePreset.entries) {
            val colors = minimalColors(theme)
            assertAtLeast(4.5, colors.onErrorContainer, colors.errorContainer, "$theme failure line")
            val pane = xdGlass(theme).pane.compositeOver(xdGlass(theme).backdrop)
            assertAtLeast(4.5, colors.onSurfaceVariant, pane, "$theme connecting line")
        }
    }

    private fun assertAtLeast(minimum: Double, foreground: Color, background: Color, what: String) {
        val lighter = maxOf(foreground.luminance(), background.luminance())
        val darker = minOf(foreground.luminance(), background.luminance())
        val ratio = (lighter + 0.05) / (darker + 0.05)
        assertTrue("$what: $ratio < $minimum", ratio >= minimum)
    }
}
