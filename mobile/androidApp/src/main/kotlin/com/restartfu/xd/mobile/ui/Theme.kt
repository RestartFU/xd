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

/** Legacy accent preferences remain readable for compatibility. */
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
    DARK("dark", "Dark", Color(0xFF1B1B1B)),
    LIGHT("light", "Light", Color(0xFFF4F4F4)),
    ;

    public companion object {
        public fun fromKey(key: String?): MinimalThemePreset =
            entries.firstOrNull { it.key == key } ?: DARK
    }
}

/**
 * Colours Material has no role for: the status signals that say what a
 * session's agent is doing, links, hints, and diff backgrounds.
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
        working = Color(0xFF8DCEB0), waiting = Color(0xFFE2C28D),
        done = Color(0xFFC2C2C2), rule = Color(0xFF393939),
        link = Color(0xFFDEDEDE), faint = Color(0xFFABABAB),
        diffAdded = Color(0xFF263B30), diffRemoved = Color(0xFF462C30),
    )
    MinimalThemePreset.LIGHT -> XdSignals(
        working = Color(0xFF226347), waiting = Color(0xFF745319),
        done = Color(0xFF555555), rule = Color(0xFFD4D4D4),
        link = Color(0xFF444444), faint = Color(0xFF606060),
        diffAdded = Color(0xFFE0EEE5), diffRemoved = Color(0xFFF4E2E5),
    )
}

public val LocalXdSignals = staticCompositionLocalOf { xdSignals(MinimalThemePreset.DARK) }

/** Neutral charcoal and silver expressed through complete Material color roles. */
public fun minimalColors(theme: MinimalThemePreset): ColorScheme = when (theme) {
    MinimalThemePreset.DARK -> darkColorScheme(
        primary = Color(0xFFDEDEDE), onPrimary = Color(0xFF1B1B1B),
        primaryContainer = Color(0xFF383838), onPrimaryContainer = Color(0xFFF1F1F1),
        secondary = Color(0xFFB1B1B1), onSecondary = Color(0xFF1B1B1B),
        secondaryContainer = Color(0xFF383838), onSecondaryContainer = Color(0xFFF1F1F1),
        tertiary = Color(0xFFC2C2C2), onTertiary = Color(0xFF1B1B1B),
        tertiaryContainer = Color(0xFF383838), onTertiaryContainer = Color(0xFFF1F1F1),
        background = Color(0xFF101010), onBackground = Color(0xFFF1F1F1),
        surface = Color(0xFF1B1B1B), onSurface = Color(0xFFF1F1F1),
        surfaceVariant = Color(0xFF303030), onSurfaceVariant = Color(0xFFB1B1B1),
        surfaceContainerLowest = Color(0xFF101010), surfaceContainerLow = Color(0xFF202020),
        surfaceContainer = Color(0xFF2A2A2A), surfaceContainerHigh = Color(0xFF303030),
        surfaceContainerHighest = Color(0xFF383838), surfaceBright = Color(0xFF383838),
        surfaceDim = Color(0xFF1B1B1B), inverseSurface = Color(0xFFF1F1F1),
        inverseOnSurface = Color(0xFF202020), inversePrimary = Color(0xFF363636),
        outline = Color(0xFF858585), outlineVariant = Color(0xFF414141),
        scrim = Color(0xFF000000), error = Color(0xFFEFA7AD), onError = Color(0xFF462C30),
        errorContainer = Color(0xFF462C30), onErrorContainer = Color(0xFFF6D9DC),
    )
    MinimalThemePreset.LIGHT -> lightColorScheme(
        primary = Color(0xFF363636), onPrimary = Color(0xFFFFFFFF),
        primaryContainer = Color(0xFFE4E4E4), onPrimaryContainer = Color(0xFF202020),
        secondary = Color(0xFF5A5A5A), onSecondary = Color(0xFFFFFFFF),
        secondaryContainer = Color(0xFFFFFFFF), onSecondaryContainer = Color(0xFF202020),
        tertiary = Color(0xFF444444), onTertiary = Color(0xFFFFFFFF),
        tertiaryContainer = Color(0xFFE4E4E4), onTertiaryContainer = Color(0xFF202020),
        background = Color(0xFFF4F4F4), onBackground = Color(0xFF202020),
        surface = Color(0xFFFAFAFA), onSurface = Color(0xFF202020),
        surfaceVariant = Color(0xFFE4E4E4), onSurfaceVariant = Color(0xFF5A5A5A),
        surfaceContainerLowest = Color(0xFFFFFFFF), surfaceContainerLow = Color(0xFFF1F1F1),
        surfaceContainer = Color(0xFFFFFFFF), surfaceContainerHigh = Color(0xFFE8E8E8),
        surfaceContainerHighest = Color(0xFFE4E4E4), surfaceBright = Color(0xFFFFFFFF),
        surfaceDim = Color(0xFFE4E4E4), inverseSurface = Color(0xFF202020),
        inverseOnSurface = Color(0xFFFAFAFA), inversePrimary = Color(0xFFDEDEDE),
        outline = Color(0xFF7D7D7D), outlineVariant = Color(0xFFD0D0D0),
        scrim = Color(0xFF000000), error = Color(0xFFA13242), onError = Color(0xFFFFFFFF),
        errorContainer = Color(0xFFF4E2E5), onErrorContainer = Color(0xFF701D29),
    )
}

@OptIn(ExperimentalTextApi::class)
private fun inter(weight: FontWeight) = Font(
    R.font.inter,
    weight = weight,
    variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
)

/** Inter, the desktop's face, carried through Material's type scale. */
public val XdFontFamily: FontFamily = FontFamily(
    inter(FontWeight.Normal),
    inter(FontWeight.Medium),
    inter(FontWeight.SemiBold),
    inter(FontWeight.Bold),
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
        titleMedium = base.titleMedium.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.Medium),
        titleSmall = base.titleSmall.copy(fontFamily = XdFontFamily, fontWeight = FontWeight.Medium),
        bodyLarge = base.bodyLarge.copy(fontFamily = XdFontFamily),
        bodyMedium = base.bodyMedium.copy(fontFamily = XdFontFamily),
        bodySmall = base.bodySmall.copy(fontFamily = XdFontFamily),
        labelLarge = base.labelLarge.copy(fontFamily = XdFontFamily),
        labelMedium = base.labelMedium.copy(fontFamily = XdFontFamily),
        labelSmall = base.labelSmall.copy(fontFamily = XdFontFamily),
    )
}
