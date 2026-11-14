package com.restartfu.xd.mobile.ui

import android.os.Build
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import androidx.compose.ui.unit.min
import kotlin.math.roundToInt

/**
 * Which layout a window gets, from Material's window size classes on both
 * axes. Width alone would give a phone on its side the tablet's stacked
 * header rows in a window 360dp tall, so the height counts too.
 */
@Immutable
internal data class WindowLayout(val widthDp: Int, val heightDp: Int) {
    /** Medium width and up: the destinations sit in a rail at the side, which costs no height. */
    val rail: Boolean get() = widthDp >= MEDIUM_WIDTH_DP

    /**
     * Room for the tablet's chat header, with the session's chats as a row of
     * tabs. A compact height never gets it: the chat keeps the phone's one bar
     * and one-row composer, so the conversation keeps what height there is.
     */
    val tabletChat: Boolean get() = rail && heightDp >= COMPACT_HEIGHT_DP

    /** Expanded width: Changes, Files and Terminal open beside the chat instead of in its place. */
    val sideBySide: Boolean get() = tabletChat && widthDp >= EXPANDED_WIDTH_DP

    companion object {
        const val MEDIUM_WIDTH_DP = 600
        const val EXPANDED_WIDTH_DP = 840
        const val COMPACT_HEIGHT_DP = 480
    }
}

/**
 * The window's layout, from the room it really has for the app: its size,
 * split-screen included, less the system bars and any cutout, which the
 * headers, composer and sides keep clear of. From Android 15 the size the
 * system reports includes those bars, so they are taken off here; before it,
 * the reported size already left them out.
 */
@Composable
internal fun rememberWindowLayout(): WindowLayout {
    val configuration = LocalConfiguration.current
    val density = LocalDensity.current
    val direction = LocalLayoutDirection.current
    val bars = WindowInsets.systemBars.union(WindowInsets.displayCutout)
    val (barsWidth, barsHeight) = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
        with(density) {
            (bars.getLeft(density, direction) + bars.getRight(density, direction)).toDp() to
                (bars.getTop(density) + bars.getBottom(density)).toDp()
        }
    } else {
        0.dp to 0.dp
    }
    return windowLayoutOf(configuration.screenWidthDp, configuration.screenHeightDp, barsWidth, barsHeight)
}

/** The layout of a window [widthDp] by [heightDp] whose bars take [barsWidth] and [barsHeight] of it. */
internal fun windowLayoutOf(widthDp: Int, heightDp: Int, barsWidth: Dp, barsHeight: Dp): WindowLayout =
    WindowLayout(
        (widthDp - barsWidth.value.roundToInt()).coerceAtLeast(0),
        (heightDp - barsHeight.value.roundToInt()).coerceAtLeast(0),
    )

/** The widest a column of text is drawn: past it, lines grow too long to read back along. */
internal val ReadingMeasure = 720.dp

/** The narrowest a tool pane beside the chat is useful at. */
private val MinToolPane = 400.dp

/**
 * The padding at each side of a reading column in [width]: [edge] in a narrow
 * window, more in a wide one so the column stops at [ReadingMeasure] and sits
 * in the middle. The list stays as wide as its window, so it scrolls from its margins too.
 */
internal fun readingInset(width: Dp, edge: Dp): Dp = max(edge, (width - ReadingMeasure) / 2)

/**
 * How wide the chat is with a tool open beside it in [available]: its reading
 * measure where the tool still has room, and never less than half, so a window
 * just wide enough for two panes splits evenly.
 */
internal fun chatPaneWidth(available: Dp, edge: Dp): Dp =
    max(available / 2, min(ReadingMeasure + edge * 2, available - MinToolPane))

/** Clear of the status bar and of a display cutout at the top. */
@Composable
internal fun Modifier.safeTopPadding(): Modifier =
    windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top))

/** Clear of a display cutout or a navigation bar at either side, as a phone on its side has. */
@Composable
internal fun Modifier.safeSidesPadding(): Modifier =
    windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
