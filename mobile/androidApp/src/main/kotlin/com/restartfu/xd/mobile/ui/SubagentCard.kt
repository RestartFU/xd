package com.restartfu.xd.mobile.ui

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.material3.OutlinedButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.SubagentRun
import com.restartfu.xd.model.DisplayLabels
import com.restartfu.xd.model.SubagentState
import kotlinx.coroutines.delay

/**
 * One delegated agent: who it is and what it is doing right now, with its
 * newest steps showing while it works. Opening it shows the task it was given,
 * every step it took, and its report.
 */
@Composable
internal fun SubagentCard(
    run: SubagentRun,
    pendingAction: String? = null,
    onAction: (key: String, action: String) -> Unit = { _, _ -> },
) {
    // Keyed by the agent, not the text: the host rewrites the record as the
    // agent works, and an open card stays open through those updates.
    var expanded by rememberSaveable(run.key ?: run.marker) { mutableStateOf(false) }
    val signals = LocalXdSignals.current
    val running = run.state == SubagentState.RUNNING
    val signal = when (run.state) {
        SubagentState.RUNNING -> signals.working
        SubagentState.SUCCESS -> signals.faint
        SubagentState.FAILURE -> MaterialTheme.colorScheme.error
        SubagentState.FINISHED -> MaterialTheme.colorScheme.outline
    }
    val shape = RoundedCornerShape(14.dp)
    val preview = if (running) {
        run.activity
    } else {
        firstLine(run.summary.ifBlank { run.latest })
    }
    // A running agent counts up once a second; a finished one shows its total.
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    if (running && run.startedAtMillis != null) {
        LaunchedEffect(run.key) {
            while (true) {
                now = System.currentTimeMillis()
                delay(1_000)
            }
        }
    }
    val stats = run.statsAt(if (running) now else null)

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(shape)
            .border(
                1.dp,
                if (run.state == SubagentState.FAILURE) {
                    MaterialTheme.colorScheme.error
                } else {
                    MaterialTheme.colorScheme.outlineVariant
                },
                shape,
            )
            .background(MaterialTheme.colorScheme.surfaceContainer)
            .clickable { expanded = !expanded }
            .animateContentSize(),
    ) {
        Column(
            modifier = Modifier
                .weight(1f)
                .padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                BackendIcon(run.provider.lowercase(), size = 14.dp)
                Text(
                    run.identity,
                    fontWeight = FontWeight.Medium,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                if (run.background) StatusPill("Background", signals.faint)
                if (running) PulseDot(signal)
                Text(
                    run.status,
                    color = if (running) MaterialTheme.colorScheme.onSurfaceVariant else signal,
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            val key = run.key
            if (key != null && (run.canStop || pendingAction != null)) {
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (pendingAction != null) {
                        Text(
                            "$pendingAction…",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.labelMedium,
                        )
                    } else {
                        if (run.canBackground) {
                            OutlinedButton(
                                onClick = { onAction(key, "background") },
                                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                                modifier = Modifier.heightIn(min = 48.dp),
                            ) { Text("Background", style = MaterialTheme.typography.labelMedium) }
                        }
                        OutlinedButton(
                            onClick = { onAction(key, "stop") },
                            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 0.dp),
                            modifier = Modifier.heightIn(min = 48.dp),
                        ) {
                            Text(
                                "Stop",
                                color = MaterialTheme.colorScheme.error,
                                style = MaterialTheme.typography.labelMedium,
                            )
                        }
                    }
                }
            }
            if (run.detail.isNotBlank()) {
                Text(
                    run.detail,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = if (expanded) Int.MAX_VALUE else 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            // A phone has no room to share one line between the latest step
            // and the numbers, so each gets its own.
            if (preview.isNotBlank()) {
                Text(
                    preview,
                    color = if (running) {
                        MaterialTheme.colorScheme.onSurface
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                    style = MaterialTheme.typography.bodySmall,
                    maxLines = if (running) 1 else 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            val modelName = run.model.takeIf { it.isNotBlank() }?.let { DisplayLabels.model(it, run.provider) }
            val meta = listOfNotNull(modelName, stats.takeIf { it.isNotBlank() }).joinToString(" · ")
            if (meta.isNotBlank()) {
                Text(
                    meta,
                    color = signals.faint,
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (running && !expanded && run.steps.isNotEmpty()) {
                run.steps.takeLast(3).forEach { StepLine(it.tool, it.text) }
            }
            if (expanded) {
                if (run.prompt.isNotBlank()) {
                    Section("Task")
                    Text(
                        run.prompt,
                        modifier = Modifier
                            .heightIn(max = 180.dp)
                            .verticalScroll(rememberScrollState()),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (run.steps.isNotEmpty()) {
                    val earlier = run.droppedSteps
                    Section("Steps · ${run.steps.size + earlier}")
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .background(MaterialTheme.colorScheme.background)
                            .heightIn(max = 240.dp)
                            .verticalScroll(rememberScrollState())
                            .padding(8.dp),
                    ) {
                        if (earlier > 0) {
                            Text(
                                "$earlier earlier steps not kept",
                                color = signals.faint,
                                style = MaterialTheme.typography.labelSmall,
                            )
                        }
                        run.steps.forEach { StepLine(it.tool, it.text) }
                    }
                }
                val report = run.summary.ifBlank { run.latest }
                if (report.isNotBlank()) {
                    Section(if (run.summary.isNotBlank()) "Report" else "Latest message")
                    Text(
                        report,
                        color = MaterialTheme.colorScheme.onSurface,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (run.prompt.isBlank() && run.steps.isEmpty() && report.isBlank() &&
                    run.detail.isBlank()
                ) {
                    Text(
                        "No task details reported.",
                        color = signals.faint,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }
}

@Composable
private fun Section(label: String) {
    Text(
        label,
        modifier = Modifier.padding(top = 6.dp),
        color = LocalXdSignals.current.faint,
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
    )
}

@Composable
private fun StepLine(tool: String, text: String) {
    Row {
        Text(
            tool,
            modifier = Modifier.width(52.dp),
            color = LocalXdSignals.current.faint,
            fontFamily = XdMonoFamily,
            style = MaterialTheme.typography.labelSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            text,
            modifier = Modifier.weight(1f),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            fontFamily = XdMonoFamily,
            style = MaterialTheme.typography.labelSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** The first line of some Markdown as plain text: no markers, emphasis, or code ticks. */
private fun firstLine(text: String): String = text.lineSequence()
    .map { it.trim().trimStart('#', '-', '*', '>', ' ') }
    .firstOrNull { it.isNotEmpty() && !it.startsWith("```") }
    .orEmpty()
    .replace("**", "")
    .replace("__", "")
    .replace("`", "")
