package com.restartfu.xd.mobile.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.XdClient
import com.restartfu.xd.model.PipelineRun
import com.restartfu.xd.model.TurnTiming
import com.restartfu.xd.protocol.WorkflowJobReply
import com.restartfu.xd.protocol.WorkflowStatusReply
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay

private const val WORKFLOW_POLL_MILLIS = 10_000L
private const val WORKFLOW_RETRY_MILLIS = 60_000L

private data class PipelineSnapshot(
    val status: WorkflowStatusReply? = null,
    val error: Boolean = false,
)

@Composable
internal fun PipelineCard(
    run: PipelineRun,
    client: XdClient,
) {
    val context = LocalContext.current
    val snapshot by produceState(
        initialValue = PipelineSnapshot(),
        key1 = run.marker,
    ) {
        while (true) {
            try {
                val status = client.workflowStatus(run.marker)
                value = PipelineSnapshot(status = status)
                if (status.state == "completed") break
                delay(WORKFLOW_POLL_MILLIS)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Throwable) {
                value = PipelineSnapshot(error = true)
                delay(WORKFLOW_RETRY_MILLIS)
            }
        }
    }

    val status = snapshot.status
    val terminal = status?.state == "completed"
    val running = !terminal && !snapshot.error
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable {
                runCatching {
                    context.startActivity(
                        Intent(Intent.ACTION_VIEW, Uri.parse(run.url)),
                    )
                }
            },
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant,
        ),
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                when {
                    running -> CircularProgressIndicator(
                        modifier = Modifier.size(18.dp),
                        strokeWidth = 2.dp,
                    )
                    snapshot.error -> Text(
                        "!",
                        color = MaterialTheme.colorScheme.error,
                        fontWeight = FontWeight.Bold,
                    )
                    else -> Text(
                        workflowIcon(status?.conclusion),
                        color = workflowColour(status?.conclusion),
                        fontWeight = FontWeight.Bold,
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text(
                    status?.name?.ifBlank { null } ?: "Pipeline",
                    modifier = Modifier.weight(1f),
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                elapsedText(
                    status?.startedAt,
                    status?.completedAt,
                    terminal,
                )?.let { elapsed ->
                    Text(
                        elapsed,
                        color = MaterialTheme.colorScheme.outline,
                        style = MaterialTheme.typography.labelMedium,
                    )
                    Spacer(Modifier.width(8.dp))
                }
                if (terminal) {
                    Text(
                        workflowLabel(status?.conclusion),
                        color = workflowColour(status?.conclusion),
                        style = MaterialTheme.typography.labelMedium,
                    )
                } else if (snapshot.error) {
                    Text(
                        "Status unavailable",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
            Text(
                "${run.repository} · #${run.id}",
                color = MaterialTheme.colorScheme.outline,
                style = MaterialTheme.typography.labelSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (status != null) {
                if (status.jobs.isEmpty()) {
                    Text(
                        "No jobs reported yet",
                        color = MaterialTheme.colorScheme.outline,
                        style = MaterialTheme.typography.bodySmall,
                    )
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        status.jobs.forEach { job -> PipelineJob(job) }
                    }
                }
            }
        }
    }
}

@Composable
private fun PipelineJob(job: WorkflowJobReply) {
    val terminal = job.state == "completed"
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp, vertical = 6.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (terminal) {
                Text(
                    workflowIcon(job.conclusion),
                    color = workflowColour(job.conclusion),
                    fontWeight = FontWeight.Bold,
                )
            } else {
                CircularProgressIndicator(
                    modifier = Modifier.size(16.dp),
                    strokeWidth = 2.dp,
                )
            }
            Spacer(Modifier.width(8.dp))
            Text(
                job.name,
                modifier = Modifier.weight(1f),
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            elapsedText(job.startedAt, job.completedAt, terminal)?.let { elapsed ->
                Text(
                    elapsed,
                    color = MaterialTheme.colorScheme.outline,
                    style = MaterialTheme.typography.labelSmall,
                )
                Spacer(Modifier.width(8.dp))
            }
            if (terminal) {
                Text(
                    workflowLabel(job.conclusion),
                    color = workflowColour(job.conclusion),
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
        job.log?.takeIf(String::isNotBlank)?.let { activity ->
            Text(
                activity,
                color = MaterialTheme.colorScheme.outline,
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/**
 * How long a run or job has been going, or took.
 *
 * The daemon reports instants rather than an elapsed count, so a running row
 * keeps counting between the ten-second polls instead of stepping in jumps. A
 * finished row without a finish time shows nothing: a climbing count on
 * something already over reads worse than no count at all.
 */
@Composable
private fun elapsedText(
    startedAt: Long?,
    completedAt: Long?,
    terminal: Boolean,
): String? {
    val running = startedAt != null && completedAt == null && !terminal
    val live = elapsedSeconds(if (running) startedAt!! * 1000 else null)
    return when {
        startedAt == null -> null
        completedAt != null -> TurnTiming.duration(completedAt - startedAt)
        terminal -> null
        else -> TurnTiming.duration(live)
    }
}

private fun workflowLabel(conclusion: String?): String = when (conclusion) {
    "success" -> "Passed"
    "failure" -> "Failed"
    "cancelled" -> "Cancelled"
    "timed_out" -> "Timed out"
    "action_required" -> "Action required"
    "startup_failure" -> "Startup failed"
    "skipped" -> "Skipped"
    "stale" -> "Stale"
    "neutral" -> "Neutral"
    else -> "Completed"
}

private fun workflowIcon(conclusion: String?): String = when (conclusion) {
    "success" -> "✓"
    "failure", "timed_out", "startup_failure" -> "✕"
    else -> "•"
}

// The desktop's .xd-workflow-success and .xd-workflow-failure, so a run reads
// the same colour on both.
private val WORKFLOW_SUCCESS = Color(0xFF8FF0A4)
private val WORKFLOW_FAILURE = Color(0xFFFF7B63)

@Composable
private fun workflowColour(conclusion: String?) = when (conclusion) {
    "success" -> WORKFLOW_SUCCESS
    "failure", "timed_out", "startup_failure" -> WORKFLOW_FAILURE
    else -> MaterialTheme.colorScheme.outline
}
