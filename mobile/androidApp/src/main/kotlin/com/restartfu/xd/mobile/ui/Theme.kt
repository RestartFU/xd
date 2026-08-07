package com.restartfu.xd.mobile.ui

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.ui.graphics.Color

/**
 * The desktop's colours, so the two clients look like one program.
 *
 * Material's own dark scheme is built around a violet, which is nothing to do
 * with anything else here: put the phone beside the desktop and they read as
 * two applications that happen to show the same chats. These values are the
 * desktop palette — near-black surfaces, one blue accent, and the same
 * greens and reds the desktop uses for state.
 *
 * Dark only, and not because a light scheme would be hard: the desktop has no
 * light mode, and a phone that turned white at sunset would be the same
 * mismatch again from the other side.
 */
public enum class AccentPreset(
    val key: String,
    val label: String,
    val color: Color,
    val onColor: Color,
    val container: Color,
) {
    BLUE(
        key = "blue",
        label = "Blue",
        color = Color(0xFF3584E4),
        onColor = Color(0xFFFFFFFF),
        container = Color(0xFF12263D),
    ),
    PURPLE(
        key = "purple",
        label = "Purple",
        color = Color(0xFFB18CFF),
        onColor = Color(0xFF17121F),
        container = Color(0xFF2A1A47),
    ),
    GREEN(
        key = "green",
        label = "Green",
        color = Color(0xFF65D18C),
        onColor = Color(0xFF0B2114),
        container = Color(0xFF123822),
    ),
    ORANGE(
        key = "orange",
        label = "Orange",
        color = Color(0xFFF2A65A),
        onColor = Color(0xFF2A1708),
        container = Color(0xFF452A11),
    ),
    PINK(
        key = "pink",
        label = "Pink",
        color = Color(0xFFF08AB6),
        onColor = Color(0xFF2D0F1C),
        container = Color(0xFF451C31),
    ),
    RED(
        key = "red",
        label = "Red",
        color = Color(0xFFFF7B63),
        onColor = Color(0xFF2A0B07),
        container = Color(0xFF4A1F1A),
    ),
    ;

    public companion object {
        public fun fromKey(key: String?): AccentPreset =
            entries.firstOrNull { it.key == key } ?: BLUE
    }
}

public fun xdColors(accent: AccentPreset = AccentPreset.BLUE): ColorScheme =
    darkColorScheme(
        // --window-bg-color and the configurable accent.
        primary = accent.color,
        onPrimary = accent.onColor,
        primaryContainer = accent.container,
        onPrimaryContainer = accent.color,

        // Nothing on the desktop is a second accent, so these stay neutral rather
        // than inventing one Material would otherwise fill in with violet.
        secondary = Color(0xFFB6B9BB),
        onSecondary = Color(0xFF101013),
        secondaryContainer = Color(0xFF16161B),
        onSecondaryContainer = Color(0xFFF2F2F4),
        tertiary = Color(0xFF8FF0A4),
        onTertiary = Color(0xFF06210F),
        tertiaryContainer = Color(0xFF11301C),
        onTertiaryContainer = Color(0xFF8FF0A4),

        background = Color(0xFF000000),
        onBackground = Color(0xFFF2F2F4),
        surface = Color(0xFF0A0A0C),
        onSurface = Color(0xFFF2F2F4),
        // --card-bg-color, which is what a card is on the desktop.
        surfaceVariant = Color(0xFF101013),
        onSurfaceVariant = Color(0xFFC0BFBC),
        surfaceContainerLowest = Color(0xFF000000),
        surfaceContainerLow = Color(0xFF060607),
        surfaceContainer = Color(0xFF0A0A0C),
        surfaceContainerHigh = Color(0xFF101013),
        // --popover-bg-color: what floats above everything else.
        surfaceContainerHighest = Color(0xFF141416),
        surfaceBright = Color(0xFF16161B),
        surfaceDim = Color(0xFF000000),
        inverseSurface = Color(0xFFF2F2F4),
        inverseOnSurface = Color(0xFF0A0A0C),

        // alpha(#ffffff, 0.07) over black, which is every border on the desktop.
        outline = Color(0xFF8B8E8F),
        outlineVariant = Color(0xFF1C1C1F),
        scrim = Color(0xFF000000),

        error = Color(0xFFFF7B63),
        onError = Color(0xFF2A0B07),
        errorContainer = Color(0xFF26090B),
        onErrorContainer = Color(0xFFFFB4AB),
    )

/** The default desktop-matching palette for callers that do not need settings. */
public val XdColors: ColorScheme = xdColors()
