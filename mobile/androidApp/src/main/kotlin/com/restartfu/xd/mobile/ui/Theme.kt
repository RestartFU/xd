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
 * ones in `Xd::UI::STYLE` — near-black surfaces, one blue accent, and the same
 * greens and reds the desktop uses for state.
 *
 * Dark only, and not because a light scheme would be hard: the desktop has no
 * light mode, and a phone that turned white at sunset would be the same
 * mismatch again from the other side.
 */
public val XdColors: ColorScheme = darkColorScheme(
    // --window-bg-color and the accent from .suggested-action.
    primary = Color(0xFF3584E4),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFF12263D),
    onPrimaryContainer = Color(0xFF9FC9F7),

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
