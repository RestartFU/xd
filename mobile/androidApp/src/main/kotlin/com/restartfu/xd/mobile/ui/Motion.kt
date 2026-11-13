package com.restartfu.xd.mobile.ui

import androidx.compose.animation.ContentTransform
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

private const val ENTER_MILLIS = 220
private const val EXIT_MILLIS = 180
private const val FADE_OUT_MILLIS = 90
private const val FADE_IN_MILLIS = 210
