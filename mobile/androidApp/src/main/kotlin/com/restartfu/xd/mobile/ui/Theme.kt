package com.restartfu.xd.mobile.ui

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
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

/** The same complete palettes exposed by the minimal desktop. */
public enum class MinimalThemePreset(
    val key: String,
    val label: String,
    val preview: Color,
) {
    DARK("dark", "Dark", Color(0xFF202020)),
    LIGHT("light", "Light", Color(0xFFF7F7F5)),
    WARM("warm", "Warm", Color(0xFF201711)),
    NORD("nord", "Nord", Color(0xFF3B4252)),
    DRACULA("dracula", "Dracula", Color(0xFF343746)),
    ;

    public companion object {
        public fun fromKey(key: String?): MinimalThemePreset =
            entries.firstOrNull { it.key == key } ?: DARK
    }
}

public fun minimalColors(theme: MinimalThemePreset): ColorScheme = when (theme) {
    MinimalThemePreset.DARK -> darkColorScheme(
        primary = Color(0xFFF07A55),
        onPrimary = Color(0xFF1B0E09),
        primaryContainer = Color(0xFF2B201C),
        onPrimaryContainer = Color(0xFFF1F1F1),
        background = Color(0xFF11110F),
        onBackground = Color(0xFFF1F1F1),
        surface = Color(0xFF202020),
        onSurface = Color(0xFFF1F1F1),
        surfaceContainerLow = Color(0xFF181818),
        surfaceContainerHigh = Color(0xFF282826),
        onSurfaceVariant = Color(0xFFA0A0A0),
        outline = Color(0xFF704333),
        outlineVariant = Color(0xFF303030),
        error = Color(0xFFC2524A),
    )
    MinimalThemePreset.LIGHT -> lightColorScheme(
        primary = Color(0xFFE96A43),
        onPrimary = Color(0xFF2A1008),
        primaryContainer = Color(0xFFFDF0EB),
        onPrimaryContainer = Color(0xFF202020),
        background = Color(0xFFF7F7F5),
        onBackground = Color(0xFF202020),
        surface = Color(0xFFFAFAFA),
        onSurface = Color(0xFF202020),
        surfaceContainerLow = Color(0xFFFFFFFF),
        surfaceContainerHigh = Color(0xFFF3F3F1),
        onSurfaceVariant = Color(0xFF6F6F6F),
        outline = Color(0xFFF1D2CB),
        outlineVariant = Color(0xFFEBEBEB),
        error = Color(0xFFB34238),
    )
    MinimalThemePreset.WARM -> darkColorScheme(
        primary = Color(0xFFEE7650),
        onPrimary = Color(0xFF1C0F08),
        primaryContainer = Color(0xFF352219),
        onPrimaryContainer = Color(0xFFF7EEE7),
        background = Color(0xFF17110D),
        onBackground = Color(0xFFF7EEE7),
        surface = Color(0xFF201711),
        onSurface = Color(0xFFF7EEE7),
        surfaceContainerLow = Color(0xFF100B08),
        surfaceContainerHigh = Color(0xFF2C2018),
        onSurfaceVariant = Color(0xFFBBA89A),
        outline = Color(0xFF764733),
        outlineVariant = Color(0xFF49362A),
        error = Color(0xFFE05E55),
    )
    MinimalThemePreset.NORD -> darkColorScheme(
        primary = Color(0xFF88C0D0),
        onPrimary = Color(0xFF172027),
        primaryContainer = Color(0xFF354253),
        onPrimaryContainer = Color(0xFFECEFF4),
        background = Color(0xFF2E3440),
        onBackground = Color(0xFFECEFF4),
        surface = Color(0xFF3B4252),
        onSurface = Color(0xFFECEFF4),
        surfaceContainerLow = Color(0xFF272C36),
        surfaceContainerHigh = Color(0xFF434C5E),
        onSurfaceVariant = Color(0xFFD8DEE9),
        outline = Color(0xFF88C0D0),
        outlineVariant = Color(0xFF4C566A),
        error = Color(0xFFBF616A),
    )
    MinimalThemePreset.DRACULA -> darkColorScheme(
        primary = Color(0xFFBD93F9),
        onPrimary = Color(0xFF21162F),
        primaryContainer = Color(0xFF3A3C4B),
        onPrimaryContainer = Color(0xFFF8F8F2),
        background = Color(0xFF282A36),
        onBackground = Color(0xFFF8F8F2),
        surface = Color(0xFF343746),
        onSurface = Color(0xFFF8F8F2),
        surfaceContainerLow = Color(0xFF21222C),
        surfaceContainerHigh = Color(0xFF44475A),
        onSurfaceVariant = Color(0xFFC7C8D8),
        outline = Color(0xFFBD93F9),
        outlineVariant = Color(0xFF44475A),
        error = Color(0xFFFF5555),
    )
}
