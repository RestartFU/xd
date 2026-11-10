package com.restartfu.xd.mobile.ui

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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.AgentChatStatus
import com.restartfu.xd.model.AgentOutcome
import com.restartfu.xd.model.AgentReport
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.SubagentRun
import com.restartfu.xd.model.agentChatStatus

/** The colour each status of an agent chat is told in, as on the desktop. */
@Composable
private fun AgentChatStatus.tone(): Color {
    val signals = LocalXdSignals.current
    return when (this) {
        AgentChatStatus.RUNNING -> signals.working
        AgentChatStatus.NEEDS_YOU -> signals.waiting
        AgentChatStatus.DONE -> signals.done
        AgentChatStatus.FAILED -> MaterialTheme.colorScheme.error
        AgentChatStatus.PAUSED, AgentChatStatus.STOPPED -> signals.faint
    }
}

/**
 * One agent this chat's agent started with xd's start_agent tool: its
 * assistant with a status dot, its name and latest word, and how it stands.
 * The row is its chat's way in, so a tap opens that chat.
 */
@Composable
internal fun AgentChatRow(run: SubagentRun, chat: ChatSummary?, onOpen: () -> Unit) {
    val signals = LocalXdSignals.current
    val status = agentChatStatus(run, chat)
    val tone = status.tone()
    val title = chat?.title?.takeIf { it.isNotBlank() && it != "New Chat" }
        ?: run.identity.takeIf { it.isNotBlank() && it != "Agent" }
        ?: firstLine(run.prompt)
    val detail = when (status) {
        AgentChatStatus.RUNNING, AgentChatStatus.PAUSED -> firstLine(run.prompt)
        else -> firstLine(run.summary.ifBlank { run.prompt })
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 2.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onOpen)
            .heightIn(min = 56.dp)
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(28.dp)) {
            Box(
                Modifier
                    .size(28.dp)
                    .clip(CircleShape)
                    .border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape)
                    .background(MaterialTheme.colorScheme.surfaceContainer),
                contentAlignment = Alignment.Center,
            ) { BackendIcon(run.provider.lowercase(), size = 14.dp) }
            Box(
                Modifier
                    .align(Alignment.BottomEnd)
                    .offset(x = 2.dp, y = 2.dp)
                    .size(11.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.background)
                    .padding(2.dp)
                    .clip(CircleShape)
                    .background(tone),
            )
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                title,
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (detail.isNotEmpty()) Text(
                detail,
                style = MaterialTheme.typography.labelMedium,
                color = if (status == AgentChatStatus.FAILED) MaterialTheme.colorScheme.error else signals.faint,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(status.label, style = MaterialTheme.typography.labelMedium, color = tone)
        Icon(
            painterResource(R.drawable.ic_chevron_right),
            contentDescription = "Open $title",
            tint = signals.faint,
            modifier = Modifier.size(14.dp),
        )
    }
}

/**
 * The message xd sent this chat when one of its agents ended a turn, as one
 * quiet line that opens that agent's chat. Its reply is on the agent's row and
 * in its chat.
 */
@Composable
internal fun AgentReportLine(report: AgentReport, chat: ChatSummary?, onOpen: () -> Unit) {
    val signals = LocalXdSignals.current
    val title = chat?.title?.takeIf(String::isNotBlank) ?: report.title.ifBlank { "An agent" }
    val (verb, tone) = when (report.outcome) {
        AgentOutcome.DONE -> "finished" to signals.done
        AgentOutcome.FAILED -> "stopped with an error" to MaterialTheme.colorScheme.error
        AgentOutcome.WAITING -> "is asking a question" to signals.waiting
    }
    Row(
        modifier = Modifier.fillMaxWidth().padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.weight(1f).height(1.dp).background(signals.rule))
        Row(
            modifier = Modifier
                .clip(CircleShape)
                .clickable(onClick = onOpen)
                .heightIn(min = 32.dp)
                .padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(Modifier.size(6.dp).clip(CircleShape).background(tone))
            Text(
                "$title $verb",
                style = MaterialTheme.typography.labelMedium,
                color = signals.faint,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 260.dp),
            )
        }
        Box(Modifier.weight(1f).height(1.dp).background(signals.rule))
    }
}

/** The one way back from an agent's chat to the chat that started it. */
@Composable
internal fun StartedByPill(parent: ChatSummary, onOpen: () -> Unit) {
    val signals = LocalXdSignals.current
    Box(Modifier.fillMaxWidth().padding(bottom = 8.dp), contentAlignment = Alignment.Center) {
        Row(
            modifier = Modifier
                .clip(CircleShape)
                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape)
                .clickable(onClick = onOpen)
                .heightIn(min = 32.dp)
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text("Started by", style = MaterialTheme.typography.labelMedium, color = signals.faint)
            Text(
                parent.title.ifBlank { "New Chat" },
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.widthIn(max = 240.dp),
            )
        }
    }
}
