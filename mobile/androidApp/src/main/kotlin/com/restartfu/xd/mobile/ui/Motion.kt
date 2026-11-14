package com.restartfu.xd.mobile.ui

import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.ContentTransform
import androidx.compose.animation.core.SeekableTransitionState
import androidx.compose.animation.core.Transition
import androidx.compose.animation.core.rememberTransition
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameNanos
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith

/*
 * The app's page transitions: Material's two patterns and nothing else. They
 * run on Compose's animation clock, which follows the system's animator
 * duration scale, and [pageMotion] drops them outright while "Remove
 * animations" is on.
 */

/** Material's shared axis along X: stepping into a page, or back out of it. */
internal fun sharedAxisEnter(forward: Boolean): EnterTransition =
    slideInHorizontally(tween(ENTER_MILLIS)) { width -> if (forward) width / 5 else -width / 5 } +
        fadeIn(tween(ENTER_MILLIS))

internal fun sharedAxisExit(forward: Boolean): ExitTransition =
    slideOutHorizontally(tween(EXIT_MILLIS)) { width -> if (forward) -width / 5 else width / 5 } +
        fadeOut(tween(EXIT_MILLIS))

internal fun sharedAxisX(forward: Boolean): ContentTransform =
    sharedAxisEnter(forward).togetherWith(sharedAxisExit(forward))

/** Material's fade through: between pages that are neither inside nor beside one another. */
internal fun fadeThrough(): ContentTransform =
    (fadeIn(tween(FADE_IN_MILLIS, delayMillis = FADE_OUT_MILLIS)) +
        scaleIn(tween(FADE_IN_MILLIS, delayMillis = FADE_OUT_MILLIS), initialScale = 0.92f))
        .togetherWith(fadeOut(tween(FADE_OUT_MILLIS)))

/** A hard cut, as every transition is while animations are off. */
internal val NoTransition: ContentTransform = EnterTransition.None.togetherWith(ExitTransition.None)

/** [transition] without its size animation, or a cut when [still]. */
internal fun pageMotion(still: Boolean, transition: ContentTransform): ContentTransform =
    if (still) NoTransition else ContentTransform(transition.targetContentEnter, transition.initialContentExit, sizeTransform = null)

/**
 * A page transition the Back gesture can drive. It follows [page], the page
 * the app's state says, with whatever spec it is drawn with; while a back
 * gesture is held, [PredictiveBack] moves it toward the page Back leads to by
 * the gesture's progress instead.
 */
internal class PageTransition<S>(val seekable: SeekableTransitionState<S>, val transition: Transition<S>)

@Composable
internal fun <S> rememberPageTransition(page: S, label: String): PageTransition<S> {
    val still = reducedMotion()
    val seekable = remember { SeekableTransitionState(page) }
    LaunchedEffect(page, still) {
        // From wherever a gesture left it, on to the page; a cut while animations are off.
        if (still) seekable.snapTo(page) else seekable.animateTo(page)
    }
    val transition = rememberTransition(seekable, label)
    return remember(seekable, transition) { PageTransition(seekable, transition) }
}

/**
 * Back as Android's predictive gesture: while it is held, [pages] moves
 * toward [back] by the gesture's progress, so the page being returned to
 * shows as it is dragged; letting go goes there ([onBack]), and a gesture
 * called off settles back. A plain Back press goes straight there. With
 * animations off nothing moves until Back is done, and then it cuts.
 */
@Composable
internal fun <S> PredictiveBack(pages: PageTransition<S>, enabled: Boolean, back: S, onBack: () -> Unit) {
    val still = reducedMotion()
    val scope = rememberCoroutineScope()
    val goBack by rememberUpdatedState(onBack)
    val target by rememberUpdatedState(back)
    PredictiveBackHandler(enabled) { gesture ->
        val seekable = pages.seekable
        val from = seekable.currentState
        val to = target
        val seeking = !still && from != to
        try {
            gesture.collect { event -> if (seeking) seekable.seekTo(event.progress, to) }
            goBack()
        } catch (cancelled: CancellationException) {
            if (seeking) scope.launch { seekable.settleBack(from) }
            throw cancelled
        }
    }
}

/** Runs a gesture's progress back down to nothing, then rests on [page] again. */
private suspend fun <S> SeekableTransitionState<S>.settleBack(page: S) {
    val toward = targetState
    val start = fraction
    val began = withFrameNanos { it }
    while (true) {
        val elapsed = (withFrameNanos { it } - began) / 1_000_000f
        val left = start * (1f - elapsed / SETTLE_MILLIS)
        if (left <= 0f) break
        seekTo(left, toward)
    }
    snapTo(page)
}

private const val SETTLE_MILLIS = 150f
private const val ENTER_MILLIS = 220
private const val EXIT_MILLIS = 180
private const val FADE_OUT_MILLIS = 90
private const val FADE_IN_MILLIS = 210
