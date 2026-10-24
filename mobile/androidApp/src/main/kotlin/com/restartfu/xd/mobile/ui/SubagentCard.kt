package com.restartfu.xd.mobile.ui

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.SubagentRun
import com.restartfu.xd.model.SubagentState

/**
 * One delegated agent: who it is and what it is doing right now, with its
 * newest steps showing while it works. Opening it shows the task it was given,
 * every step it took, and its report.
 */
@Composable
internal fun SubagentCard(run: SubagentRun) {
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
    val shape = RoundedCornerShape(6.dp)
    val preview = if (running) {
        run.activity
    } else {
        firstLine(run.summary.ifBlank { run.latest })
    }

    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(IntrinsicSize.Min)
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
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable { expanded = !expanded }
            .animateContentSize(),
    ) {
        Box(
            Modifier
                .width(3.dp)
                .fillMaxHeight()
                .background(signal),
        )
        Column(
            modifier = Modifier
                .weight(1f)
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    run.identity,
                    fontWeight = FontWeight.SemiBold,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                )
                if (run.model.isNotBlank()) {
                    Spacer(Modifier.width(6.dp))
                    Text(
                        run.model,
                        color = signals.faint,
                        fontFamily = XdMonoFamily,
                        style = MaterialTheme.typography.labelSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                }
                Spacer(Modifier.weight(1f))
                if (run.background) {
                    Text(
                        "BACKGROUND",
                        color = signals.faint,
                        style = MaterialTheme.typography.labelSmall,
                        modifier = Modifier
                            .border(1.dp, signals.faint, RoundedCornerShape(2.dp))
                            .padding(horizontal = 4.dp, vertical = 1.dp),
                    )
                    Spacer(Modifier.width(8.dp))
                }
                if (running) PulsingDot(signal)
                Spacer(Modifier.width(6.dp))
                Text(run.status, color = signal, style = MaterialTheme.typography.labelMedium)
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
            if (preview.isNotBlank() || run.stats.isNotBlank()) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        preview,
                        modifier = Modifier.weight(1f),
                        color = if (running) {
                            MaterialTheme.colorScheme.onSurface
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (run.stats.isNotBlank()) {
                        Spacer(Modifier.width(8.dp))
                        Text(run.stats, color = signals.faint, style = MaterialTheme.typography.labelSmall)
                    }
                }
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
                    Section("Steps · ${run.toolUses}")
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(5.dp))
                            .background(MaterialTheme.colorScheme.background)
                            .heightIn(max = 240.dp)
                            .verticalScroll(rememberScrollState())
                            .padding(8.dp),
                    ) {
                        val earlier = run.toolUses - run.steps.size
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

@Composable
private fun PulsingDot(color: Color) {
    val transition = rememberInfiniteTransition(label = "subagent")
    val alpha by transition.animateFloat(
        initialValue = 0.35f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(700), RepeatMode.Reverse),
        label = "subagent-pulse",
    )
    Box(
        Modifier
            .size(6.dp)
            .alpha(alpha)
            .clip(CircleShape)
            .background(color),
    )
}

private fun firstLine(text: String): String = text.lineSequence()
    .map { it.trim().trimStart('#', '-', '*', '>', ' ') }
    .firstOrNull { it.isNotEmpty() }
    .orEmpty()
