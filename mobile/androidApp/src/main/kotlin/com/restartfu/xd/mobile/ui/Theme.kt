package com.restartfu.xd.mobile.ui

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import com.restartfu.xd.mobile.R

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

/** The desktop's two themes. Saved names of retired themes load as Dark. */
public enum class MinimalThemePreset(
    val key: String,
    val label: String,
    val preview: Color,
) {
    DARK("dark", "Dark", Color(0xFF1E1F22)),
    LIGHT("light", "Light", Color(0xFFFFFFFF)),
    ;

    public companion object {
        public fun fromKey(key: String?): MinimalThemePreset =
            entries.firstOrNull { it.key == key } ?: DARK
    }
}

/**
 * Colours Material has no role for: the signal tabs that say what a session's
 * agent is doing, and the red ruled line under a card's title.
 */
@Immutable
public data class XdSignals(
    val working: Color,
    val waiting: Color,
    val done: Color,
    val rule: Color,
    val link: Color,
    val faint: Color,
    val diffAdded: Color,
    val diffRemoved: Color,
)

public fun xdSignals(theme: MinimalThemePreset): XdSignals = when (theme) {
    MinimalThemePreset.DARK -> XdSignals(
        working = Color(0xFF72B783),
        waiting = Color(0xFFEAA93A),
        done = Color(0xFF86A9E4),
        rule = Color(0xFFA8493C),
        link = Color(0xFF93B5E9),
        faint = Color(0xFF8B8A85),
        diffAdded = Color(0xFF1B2B20),
        diffRemoved = Color(0xFF33201E),
    )
    MinimalThemePreset.LIGHT -> XdSignals(
        working = Color(0xFF276B3C),
        waiting = Color(0xFF8C5100),
        done = Color(0xFF2B5897),
        rule = Color(0xFFC2483B),
        link = Color(0xFF2B5897),
        faint = Color(0xFF64635D),
        diffAdded = Color(0xFFE2F0E5),
        diffRemoved = Color(0xFFF8E4E1),
    )
}

public val LocalXdSignals = staticCompositionLocalOf { xdSignals(MinimalThemePreset.DARK) }

/** The desktop palette as complete Material roles, so no default violet leaks in. */
public fun minimalColors(theme: MinimalThemePreset): ColorScheme = when (theme) {
    MinimalThemePreset.DARK -> darkColorScheme(
        primary = Color(0xFFE8E5DE),
        onPrimary = Color(0xFF121314),
        primaryContainer = Color(0xFF2B2D31),
        onPrimaryContainer = Color(0xFFECEAE5),
        secondary = Color(0xFFAAA9A3),
        onSecondary = Color(0xFF121314),
        secondaryContainer = Color(0xFF27292C),
        onSecondaryContainer = Color(0xFFECEAE5),
        tertiary = Color(0xFF93B5E9),
        onTertiary = Color(0xFF121314),
        tertiaryContainer = Color(0xFF1E2A3B),
        onTertiaryContainer = Color(0xFFD6E3F7),
        background = Color(0xFF121314),
        onBackground = Color(0xFFECEAE5),
        surface = Color(0xFF17181A),
        onSurface = Color(0xFFECEAE5),
        surfaceVariant = Color(0xFF27292C),
        onSurfaceVariant = Color(0xFFAAA9A3),
        surfaceContainerLowest = Color(0xFF0E0F10),
        surfaceContainerLow = Color(0xFF17181A),
        surfaceContainer = Color(0xFF1E1F22),
        surfaceContainerHigh = Color(0xFF27292C),
        surfaceContainerHighest = Color(0xFF2B2D31),
        surfaceBright = Color(0xFF2B2D31),
        surfaceDim = Color(0xFF121314),
        inverseSurface = Color(0xFFECEAE5),
        inverseOnSurface = Color(0xFF121314),
        inversePrimary = Color(0xFF1B1C1E),
        outline = Color(0xFF55575C),
        outlineVariant = Color(0xFF313338),
        scrim = Color(0xFF000000),
        error = Color(0xFFEC7064),
        onError = Color(0xFF121314),
        errorContainer = Color(0xFF3B201E),
        onErrorContainer = Color(0xFFF6C3BD),
    )
    MinimalThemePreset.LIGHT -> lightColorScheme(
        primary = Color(0xFF1B1C1E),
        onPrimary = Color(0xFFFFFFFF),
        primaryContainer = Color(0xFFE6E5E0),
        onPrimaryContainer = Color(0xFF1B1C1E),
        secondary = Color(0xFF56554F),
        onSecondary = Color(0xFFFFFFFF),
        secondaryContainer = Color(0xFFE9E9E5),
        onSecondaryContainer = Color(0xFF1B1C1E),
        tertiary = Color(0xFF2B5897),
        onTertiary = Color(0xFFFFFFFF),
        tertiaryContainer = Color(0xFFDCE6F5),
        onTertiaryContainer = Color(0xFF13305A),
        background = Color(0xFFF7F7F5),
        onBackground = Color(0xFF1B1C1E),
        surface = Color(0xFFEEEEEB),
        onSurface = Color(0xFF1B1C1E),
        surfaceVariant = Color(0xFFE9E9E5),
        onSurfaceVariant = Color(0xFF56554F),
        surfaceContainerLowest = Color(0xFFFFFFFF),
        surfaceContainerLow = Color(0xFFF7F7F5),
        surfaceContainer = Color(0xFFFFFFFF),
        surfaceContainerHigh = Color(0xFFE9E9E5),
        surfaceContainerHighest = Color(0xFFE6E5E0),
        surfaceBright = Color(0xFFFFFFFF),
        surfaceDim = Color(0xFFE6E5E0),
        inverseSurface = Color(0xFF1B1C1E),
        inverseOnSurface = Color(0xFFF7F7F5),
        inversePrimary = Color(0xFFE8E5DE),
        outline = Color(0xFF9A9993),
        outlineVariant = Color(0xFFD7D6D0),
        scrim = Color(0xFF000000),
        error = Color(0xFFB2342A),
        onError = Color(0xFFFFFFFF),
        errorContainer = Color(0xFFF8E2DF),
        onErrorContainer = Color(0xFF5C140E),
    )
}

@OptIn(ExperimentalTextApi::class)
private fun franklin(weight: FontWeight) = Font(
    R.font.libre_franklin,
    weight = weight,
    variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
)

/** Libre Franklin, the desktop's face, carried through Material's type scale. */
public val XdFontFamily: FontFamily = FontFamily(
    franklin(FontWeight.Normal),
    franklin(FontWeight.Medium),
    franklin(FontWeight.SemiBold),
    franklin(FontWeight.Bold),
)

/** JetBrains Mono, the desktop's code face, for code, branches, and the terminal. */
@OptIn(ExperimentalTextApi::class)
public val XdMonoFamily: FontFamily = FontFamily(
    listOf(FontWeight.Normal, FontWeight.Medium, FontWeight.Bold).map { weight ->
        Font(
            R.font.jetbrains_mono,
            weight = weight,
            variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
        )
    },
)

public val XdTypography: Typography = Typography().let { base ->
    Typography(
        displayLarge = base.displayLarge.copy(fontFamily = XdFontFamily),
        displayMedium = base.displayMedium.copy(fontFamily = XdFontFamily),
        displaySmall = base.displaySmall.copy(fontFamily = XdFontFamily),
        headlineLarge = base.headlineLarge.copy(fontFamily = XdFontFamily),
        headlineMedium = base.headlineMedium.copy(fontFamily = XdFontFamily),
        headlineSmall = base.headlineSmall.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.SemiBold),
        titleLarge = base.titleLarge.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.SemiBold),
        titleMedium = base.titleMedium.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.SemiBold),
        titleSmall = base.titleSmall.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.SemiBold),
        bodyLarge = base.bodyLarge.copy(fontFamily = XdFontFamily),
        bodyMedium = base.bodyMedium.copy(fontFamily = XdFontFamily),
        bodySmall = base.bodySmall.copy(fontFamily = XdFontFamily),
        labelLarge = base.labelLarge.copy(fontFamily = XdFontFamily),
        labelMedium = base.labelMedium.copy(fontFamily = XdFontFamily),
        labelSmall = base.labelSmall.copy(fontFamily = XdFontFamily),
    )
}
