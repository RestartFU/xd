package com.restartfu.xd.mobile.ui

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.foundation.background
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
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
    /** Dark text and system bar icons on a light ground. */
    val isLight: Boolean,
) {
    DARK("dark", "Dark", isLight = false),
    LIGHT("light", "Light", isLight = true),
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
    val codexIcon: Color,
    val claudeIcon: Color,
)

public fun xdSignals(theme: MinimalThemePreset): XdSignals = when (theme) {
    MinimalThemePreset.DARK -> XdSignals(
        working = Color(0xFF63D8A8), waiting = Color(0xFFF0C06A),
        done = Color(0xFFA9ADFF), rule = Color(0xFF262626),
        link = Color(0xFFA9ADFF), faint = Color(0xFF929292),
        diffAdded = Color(0xFF142B20), diffRemoved = Color(0xFF351A20),
        codexIcon = Color.White, claudeIcon = Color(0xFFD97757),
    )
    MinimalThemePreset.LIGHT -> XdSignals(
        working = Color(0xFF17603F), waiting = Color(0xFF6C490A),
        done = Color(0xFF4045BB), rule = Color(0xFFDADDE9),
        link = Color(0xFF4045BB), faint = Color(0xFF5A6074),
        diffAdded = Color(0xFFDFF2E6), diffRemoved = Color(0xFFF8E1E6),
        codexIcon = Color(0xFF1B1E2C), claudeIcon = Color(0xFFB85C3F),
    )
}

public val LocalXdSignals = staticCompositionLocalOf { xdSignals(MinimalThemePreset.DARK) }

/**
 * Frosted glass drawn inside the app, as on the desktop, in both themes: the
 * window paints one flat backdrop, page canvases leave it showing, and headers, the rail and the
 * composer are translucent panes over it. Dialogs, cards and the terminal keep
 * their opaque roles. Nothing outside the app shows through.
 */
@Immutable
public data class XdGlass(
    val backdrop: Color,
    val pane: Color,
    val composer: Color,
    /** The hairline along a pane: light on the dark backdrop, dark on the light one. */
    val edge: Color,
)

public fun xdGlass(theme: MinimalThemePreset): XdGlass = when (theme) {
    MinimalThemePreset.DARK -> XdGlass(
        backdrop = Color(0xFF0B0B0B),
        pane = Color(0x08FFFFFF), composer = Color(0xD9232323), edge = Color(0x1FFFFFFF),
    )
    MinimalThemePreset.LIGHT -> XdGlass(
        backdrop = Color(0xFFE4E7F4),
        pane = Color(0x8CFFFFFF), composer = Color(0xD0FFFFFF), edge = Color(0x1F1B1E2C),
    )
}

public val LocalXdGlass = staticCompositionLocalOf { xdGlass(MinimalThemePreset.DARK) }

/** Paints the glass backdrop behind everything. */
public fun Modifier.glassBackdrop(glass: XdGlass): Modifier = background(glass.backdrop)

/** A page canvas: clear, so the backdrop shows. */
public val GlassPage: Color = Color.Transparent

/**
 * Everything a theme provides: Material roles, the status signals, and glass.
 * The caller paints [glassBackdrop] behind its root.
 */
@Composable
public fun XdTheme(theme: MinimalThemePreset, content: @Composable () -> Unit) {
    CompositionLocalProvider(
        LocalXdSignals provides xdSignals(theme),
        LocalXdGlass provides xdGlass(theme),
    ) {
        MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography, content = content)
    }
}

/** The app's root: the theme, the glass backdrop behind everything, and the page over it. */
@Composable
public fun XdRoot(theme: MinimalThemePreset, content: @Composable () -> Unit) {
    XdTheme(theme) {
        Surface(
            Modifier.fillMaxSize().glassBackdrop(xdGlass(theme)),
            color = GlassPage,
            // Said outright: under glass the color is clear, and Material would
            // fall back to black for content on it.
            contentColor = MaterialTheme.colorScheme.onSurface,
            content = content,
        )
    }
}

/** A header, tab bar or rail over the page: translucent. */
@Composable
public fun glassPane(): Color = LocalXdGlass.current.pane

/**
 * Something that floats over content that scrolls beneath it, such as the
 * floating tab bar. Without a backdrop blur a translucent pane would show that
 * content through it, so this is the pane already laid over the backdrop: the
 * same frosted tone, but opaque.
 */
@Composable
public fun glassOverlay(): Color = LocalXdGlass.current.overlay()

/** The pane over the backdrop, opaque. */
public fun XdGlass.overlay(): Color = pane.compositeOver(backdrop)

/** The message composer: translucent. */
@Composable
public fun glassComposer(): Color = LocalXdGlass.current.composer

/** A pane's hairline. */
@Composable
public fun glassEdge(): Color = LocalXdGlass.current.edge

/**
 * Frosted glass with an indigo accent, black (Dark) or cool grey (Light),
 * expressed through complete Material color roles.
 */
public fun minimalColors(theme: MinimalThemePreset): ColorScheme = when (theme) {
    MinimalThemePreset.DARK -> darkColorScheme(
        primary = Color(0xFF8B8FF8), onPrimary = Color(0xFF0E1016),
        primaryContainer = Color(0xFF2A2A2A), onPrimaryContainer = Color(0xFFF2F2F2),
        secondary = Color(0xFFADADAD), onSecondary = Color(0xFF0B0B0B),
        secondaryContainer = Color(0xFF2A2A2A), onSecondaryContainer = Color(0xFFF2F2F2),
        tertiary = Color(0xFFA9ADFF), onTertiary = Color(0xFF0E1016),
        tertiaryContainer = Color(0xFF2A2A2A), onTertiaryContainer = Color(0xFFF2F2F2),
        background = Color(0xFF0B0B0B), onBackground = Color(0xFFF2F2F2),
        surface = Color(0xFF0B0B0B), onSurface = Color(0xFFF2F2F2),
        surfaceVariant = Color(0xFF1F1F1F), onSurfaceVariant = Color(0xFFADADAD),
        surfaceContainerLowest = Color(0xFF050505), surfaceContainerLow = Color(0xFF131313),
        surfaceContainer = Color(0xFF161616), surfaceContainerHigh = Color(0xFF202020),
        surfaceContainerHighest = Color(0xFF2A2A2A), surfaceBright = Color(0xFF2A2A2A),
        surfaceDim = Color(0xFF0B0B0B), inverseSurface = Color(0xFFF2F2F2),
        inverseOnSurface = Color(0xFF131313), inversePrimary = Color(0xFF4B4FB8),
        outline = Color(0xFF858585), outlineVariant = Color(0xFF333333),
        scrim = Color(0xFF000000), error = Color(0xFFFF94A2), onError = Color(0xFF351A20),
        errorContainer = Color(0xFF351A20), onErrorContainer = Color(0xFFFFD9DE),
    )
    MinimalThemePreset.LIGHT -> lightColorScheme(
        primary = Color(0xFF5257D9), onPrimary = Color(0xFFFFFFFF),
        primaryContainer = Color(0xFFDFE2F4), onPrimaryContainer = Color(0xFF1B1E2C),
        secondary = Color(0xFF4B5165), onSecondary = Color(0xFFFFFFFF),
        secondaryContainer = Color(0xFFDFE2F4), onSecondaryContainer = Color(0xFF1B1E2C),
        tertiary = Color(0xFF4045BB), onTertiary = Color(0xFFFFFFFF),
        tertiaryContainer = Color(0xFFDFE2F4), onTertiaryContainer = Color(0xFF1B1E2C),
        background = Color(0xFFEEF0F9), onBackground = Color(0xFF1B1E2C),
        surface = Color(0xFFF3F4FB), onSurface = Color(0xFF1B1E2C),
        surfaceVariant = Color(0xFFECEEF7), onSurfaceVariant = Color(0xFF4B5165),
        surfaceContainerLowest = Color(0xFFFFFFFF), surfaceContainerLow = Color(0xFFF3F4FB),
        surfaceContainer = Color(0xFFF8F9FD), surfaceContainerHigh = Color(0xFFECEEF7),
        surfaceContainerHighest = Color(0xFFE4E7F2), surfaceBright = Color(0xFFFFFFFF),
        surfaceDim = Color(0xFFE4E7F2), inverseSurface = Color(0xFF1B1E2C),
        inverseOnSurface = Color(0xFFF3F4FB), inversePrimary = Color(0xFFA3A7FF),
        outline = Color(0xFF6E7488), outlineVariant = Color(0xFFD3D7E6),
        scrim = Color(0xFF000000), error = Color(0xFF97203A), onError = Color(0xFFFFFFFF),
        errorContainer = Color(0xFFFBE9ED), onErrorContainer = Color(0xFF6E1426),
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
