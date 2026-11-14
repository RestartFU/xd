package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Icon
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.CancellationException
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
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.res.painterResource
import com.restartfu.xd.mobile.R
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.clearAndSetSemantics
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
 * The mark for work still running in the background, as on the desktop: a gray
 * ring that holds still, where a working dot is green, filled, and pulses.
 */
@Composable
internal fun BackgroundRing(color: Color, modifier: Modifier = Modifier, size: Dp = 8.dp) {
    Box(
        modifier
            .size(size)
            .border(1.5.dp, color, CircleShape),
    )
}

/** Task output is fetched only while this expanded row is on screen and resumed. */
@Composable
internal fun BackgroundTasksRow(
    count: Int,
    visible: Boolean,
    load: suspend () -> List<com.restartfu.xd.protocol.BackgroundTaskOutput>,
) {
    var expanded by remember { mutableStateOf(false) }
    var tasks by remember { mutableStateOf<List<com.restartfu.xd.protocol.BackgroundTaskOutput>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(expanded, visible, lifecycle) {
        if (!expanded || !visible) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                try {
                    tasks = load()
                    error = null
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    tasks = null
                    error = "Could not load background tasks."
                }
                delay(2_500)
            }
        }
    }
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 48.dp)
                .clickable(role = Role.Button, onClickLabel = if (expanded) "Hide background tasks" else "Show background tasks") {
                    expanded = !expanded
                }
                .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
                .padding(start = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            BackgroundRing(MaterialTheme.colorScheme.onSurfaceVariant, size = 7.dp)
            Text(
                backgroundTasksLabel(count),
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Icon(
                painterResource(R.drawable.ic_chevron_down),
                contentDescription = null,
                modifier = Modifier.size(18.dp).rotate(if (expanded) 180f else 0f),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (expanded) {
            Column(
                Modifier.fillMaxWidth().padding(start = 17.dp, bottom = 8.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                val current = tasks
                if (error != null || current == null || current.isEmpty()) {
                    Text(
                        error ?: if (current == null) "Loading background tasks…" else "No background tasks running.",
                        modifier = Modifier.announced(),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                current.orEmpty().forEach { task ->
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(task.description, style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.Medium)
                        Text(
                            "${task.kind} · ${TurnTiming.duration(elapsedSeconds(task.startedAt.takeIf { it > 0 }?.times(1_000)))}",
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        SelectionContainer {
                            Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(8.dp)) {
                                task.outputError?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                                if (task.outputError == null && task.lines.isEmpty()) {
                                    Text("No output yet.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                                task.lines.forEach { line ->
                                    Text(
                                        line.text,
                                        style = MaterialTheme.typography.bodySmall,
                                        fontFamily = FontFamily.Monospace,
                                        color = if (line.error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                        if (task.truncated) Text("Latest output only", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
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
    // With animations off the three dots hold still, all lit.
    val still = reducedMotion()
    val lit by produceState(if (still) 3 else 0, still) {
        if (still) {
            value = 3
            return@produceState
        }
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
        // Three full stops are not words: a screen reader hears the description, or nothing.
        modifier = Modifier.clearAndSetSemantics {
            contentDescription?.let { this.contentDescription = it }
        },
        style = MaterialTheme.typography.bodySmall,
    )
}
