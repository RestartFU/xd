package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.TurnTiming
import com.restartfu.xd.model.backgroundTasksLabel
import kotlinx.coroutines.delay

/** Frames per second of the dots, matching the desktop's `Dots`. */
private const val DOT_FRAME_MILLIS = 400L
private const val DIM_ALPHA = 0.3f

/**
 * The turn in progress, shown the way the desktop shows it: an elapsed count
 * and moving dots at the foot of the transcript.
 *
 * A turn can spend a long time between visible output — thinking, or inside a
 * tool that prints nothing — and without this the phone looks like it dropped
 * the message. The composer's Queue and Cancel buttons already imply a turn is
 * running, but only to a reader who knows they change.
 */
@Composable
internal fun WorkingRow(startedAtMillis: Long?) {
    Row(
        modifier = Modifier.padding(start = 2.dp, top = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        PulseDot(LocalXdSignals.current.working)
        Text(
            TurnTiming.format("Working", elapsedSeconds(startedAtMillis)),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Dots(MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/**
 * The mark for work still running in the background, as on the desktop: a ring
 * in the working color that holds still, where a working dot is filled and
 * pulses.
 */
@Composable
internal fun BackgroundRing(color: Color, modifier: Modifier = Modifier, size: Dp = 8.dp) {
    Box(
        modifier
            .size(size)
            .border(1.5.dp, color, CircleShape),
    )
}

/**
 * A chat with no turn running whose agent still runs background agents or
 * shells, at the foot of the transcript where the working row would be.
 */
@Composable
internal fun BackgroundTasksRow(count: Int) {
    Row(
        modifier = Modifier.padding(start = 2.dp, top = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        BackgroundRing(LocalXdSignals.current.working, size = 7.dp)
        Text(
            backgroundTasksLabel(count),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/**
 * Seconds since the turn began, recomputed on the second boundary.
 *
 * Derived from the start time rather than counted up, so a turn that was
 * already running when the chat opened shows its real age: the host reports
 * `working_for` on load and the store turns it into a start time.
 */
@Composable
internal fun elapsedSeconds(startedAtMillis: Long?): Long {
    val seconds by produceState(0L, startedAtMillis) {
        if (startedAtMillis == null) {
            value = 0L
            return@produceState
        }
        while (true) {
            val elapsed = System.currentTimeMillis() - startedAtMillis
            value = elapsed / 1000
            // Wake on the boundary, so the count never sits a beat behind.
            delay(1_000L - elapsed.mod(1_000L))
        }
    }
    return seconds
}

@Composable
internal fun Dots(color: Color, contentDescription: String? = null) {
    val lit by produceState(0) {
        while (true) {
            delay(DOT_FRAME_MILLIS)
            value = (value + 1) % 4
        }
    }
    Text(
        buildAnnotatedString {
            repeat(3) { index ->
                val shade = if (index < lit) color else color.copy(alpha = DIM_ALPHA)
                withStyle(SpanStyle(color = shade)) { append(".") }
            }
        },
        modifier = Modifier.semantics {
            contentDescription?.let { this.contentDescription = it }
        },
        style = MaterialTheme.typography.bodySmall,
    )
}
