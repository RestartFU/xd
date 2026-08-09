package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.SubagentRun
import com.restartfu.xd.model.SubagentState

@Composable
internal fun SubagentCard(run: SubagentRun) {
    var expanded by rememberSaveable(run.marker) { mutableStateOf(false) }
    val statusColor = subagentColour(run.state)

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant,
        ),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clickable { expanded = !expanded }
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            Text(
                "Subagent",
                color = MaterialTheme.colorScheme.outline,
                style = MaterialTheme.typography.labelSmall,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(6.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("●", color = statusColor, style = MaterialTheme.typography.labelSmall)
                Spacer(Modifier.width(8.dp))
                Text(
                    run.identity,
                    modifier = Modifier.weight(1f),
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    run.status,
                    color = statusColor,
                    style = MaterialTheme.typography.labelMedium,
                )
                Spacer(Modifier.width(8.dp))
                Text(if (expanded) "▾" else "▸", color = MaterialTheme.colorScheme.outline)
            }
            if (expanded) {
                Spacer(Modifier.height(10.dp))
                Text(
                    run.detail.ifBlank { "No task details reported." },
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun subagentColour(state: SubagentState): Color = when (state) {
    SubagentState.RUNNING -> MaterialTheme.colorScheme.primary
    SubagentState.SUCCESS -> MaterialTheme.colorScheme.tertiary
    SubagentState.FAILURE -> MaterialTheme.colorScheme.error
    SubagentState.FINISHED -> MaterialTheme.colorScheme.outline
}
