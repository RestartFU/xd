package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.CiRun
import com.restartfu.xd.model.CiState
import com.restartfu.xd.model.TurnTiming

/**
 * The card of a CI watch an agent asked for with xd's watch_ci tool: the pull
 * request, how its checks stand overall, and each check, always shown, since
 * following them is what the card is for. It opens the pull request's checks.
 * The desktop draws the same card.
 */
@Composable
internal fun CiCard(run: CiRun, openLink: (String) -> Unit) {
    val failed = run.state == CiState.FAILED
    val shape = RoundedCornerShape(12.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 4.dp)
            .clip(shape)
            .border(
                1.dp,
                if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.outlineVariant,
                shape,
            )
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth().clickable { openLink(run.checksUrl) },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StateDot(run.state, 8)
            Spacer(Modifier.width(8.dp))
            Text("CI", style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.width(8.dp))
            Text(
                listOfNotNull(run.number?.let { "#$it" }, run.title.ifBlank { null })
                    .joinToString(" · ").ifBlank { "Pull request" },
                modifier = Modifier.weight(1f),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            ciElapsed(run)?.let { elapsed ->
                Text(
                    elapsed,
                    color = MaterialTheme.colorScheme.outline,
                    style = MaterialTheme.typography.labelMedium,
                )
                Spacer(Modifier.width(8.dp))
            }
            Text(
                when (run.state) {
                    CiState.PASSED -> "Passed"
                    CiState.FAILED -> "Failed"
                    CiState.STOPPED -> "Stopped"
                    else -> "Running"
                },
                color = stateColour(run.state),
                style = MaterialTheme.typography.labelMedium,
                fontWeight = FontWeight.SemiBold,
            )
        }
        if (run.checks.isNotEmpty()) {
            Column(
                modifier = Modifier.padding(start = 16.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                run.checks.forEach { check ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        StateDot(check.state, 6)
                        Spacer(Modifier.width(8.dp))
                        Text(
                            check.name,
                            modifier = Modifier.weight(1f),
                            fontFamily = XdMonoFamily,
                            style = MaterialTheme.typography.bodySmall,
                            color = if (check.state == CiState.FAILED) {
                                MaterialTheme.colorScheme.error
                            } else {
                                MaterialTheme.colorScheme.onSurface
                            },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Spacer(Modifier.width(8.dp))
                        Text(
                            check.label,
                            color = stateColour(check.state),
                            style = MaterialTheme.typography.labelSmall,
                        )
                    }
                }
                if (run.unlisted > 0) {
                    Text(
                        "${run.unlisted} more",
                        modifier = Modifier.padding(start = 14.dp),
                        color = MaterialTheme.colorScheme.outline,
                        style = MaterialTheme.typography.labelSmall,
                    )
                }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                run.summary,
                modifier = Modifier.weight(1f),
                color = MaterialTheme.colorScheme.outline,
                style = MaterialTheme.typography.labelSmall,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            if (run.head.isNotBlank()) {
                Spacer(Modifier.width(8.dp))
                Text(
                    "Head ${run.head}",
                    color = MaterialTheme.colorScheme.outline,
                    fontFamily = XdMonoFamily,
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
    }
}

@Composable
private fun StateDot(state: CiState, size: Int) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(stateColour(state)))
}

// The desktop's colours for the same states, so a card reads alike on both.
@Composable
private fun stateColour(state: CiState): Color = when (state) {
    CiState.RUNNING -> LocalXdSignals.current.working
    CiState.PASSED -> LocalXdSignals.current.done
    CiState.FAILED -> MaterialTheme.colorScheme.error
    CiState.STOPPED, CiState.SKIPPED -> LocalXdSignals.current.faint
}

/** How long the watch has run, or ran; a running one counts on between updates. */
@Composable
private fun ciElapsed(run: CiRun): String? {
    val started = run.startedAt ?: return null
    val finished = run.finishedAt
    val live = elapsedSeconds(if (finished == null) started * 1000 else null)
    return TurnTiming.duration(if (finished != null) finished - started else live)
}
