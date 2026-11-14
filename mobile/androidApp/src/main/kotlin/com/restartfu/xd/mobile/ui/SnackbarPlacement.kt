package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.union
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.layout.findRootCoordinates
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.offset

/*
 * Where the app's snackbar sits. It belongs to the whole window, but what is
 * at the bottom of the window changes: the keyboard, the floating bar, and in
 * a chat the composer, which a notice must never cover while the reader is
 * writing in it. A composer says how far up the window it reaches; the
 * snackbar stands above the highest of them, or else above the keyboard and
 * the navigation bar.
 */

/** How far up from the window's bottom each composer on screen reaches. */
@Stable
internal class SnackbarClearance {
    private val reaches = mutableStateMapOf<Any, Dp>()

    /** The highest composer's top, from the window's bottom; 0 with none on screen. */
    val composer: Dp get() = reaches.values.maxOrNull() ?: 0.dp

    fun reach(owner: Any, height: Dp) {
        if (reaches[owner] != height) reaches[owner] = height
    }

    fun leave(owner: Any) {
        reaches.remove(owner)
    }
}

internal val LocalSnackbarClearance = staticCompositionLocalOf { SnackbarClearance() }

/**
 * Marks what it is applied to as a composer: the snackbar keeps above its top,
 * on phones and tablets, with or without the keyboard, for as long as it is shown.
 */
@Composable
internal fun Modifier.keepsSnackbarAbove(): Modifier {
    val clearance = LocalSnackbarClearance.current
    val density = LocalDensity.current
    val owner = remember { Any() }
    DisposableEffect(clearance, owner) { onDispose { clearance.leave(owner) } }
    return onGloballyPositioned { coordinates ->
        val root = coordinates.findRootCoordinates()
        val top = root.localPositionOf(coordinates, Offset.Zero).y
        clearance.reach(owner, with(density) { (root.size.height - top).coerceAtLeast(0f).toDp() })
    }
}

/**
 * The snackbar's distance from the window's bottom: above a composer when one
 * is shown, else [above] clear of [insets], the keyboard or navigation bar
 * there. A composer already stands above the keyboard, so it is not added twice.
 */
internal fun snackbarBottom(composer: Dp, insets: Dp, above: Dp): Dp =
    if (composer > 0.dp) composer + SnackbarGap else insets + above

/**
 * Lifts the snackbar to [snackbarBottom]: over [clearance]'s composer, or
 * [above] clear of the keyboard and navigation bar. Read while laying out, so
 * the keyboard sliding up moves the snackbar without recomposing the app.
 */
@Composable
internal fun Modifier.snackbarPlacement(clearance: SnackbarClearance, above: Dp): Modifier {
    val below = WindowInsets.navigationBars.union(WindowInsets.ime)
    return layout { measurable, constraints ->
        val bottom = snackbarBottom(clearance.composer, below.getBottom(this).toDp(), above).roundToPx()
        val placeable = measurable.measure(constraints.offset(vertical = -bottom))
        layout(placeable.width, placeable.height + bottom) { placeable.place(0, 0) }
    }
}

/** Between a composer's top and a snackbar over it. */
internal val SnackbarGap = 8.dp
