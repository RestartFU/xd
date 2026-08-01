package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.model.ChatState

/** The access levels the desktop offers. Plan is the separate toggle. */
private val ACCESS = listOf(
    "read-only" to "Read only",
    "edit" to "Edit files",
    "full" to "Full access",
)

private val EFFORT_LABELS = mapOf(
    "low" to "Low",
    "medium" to "Medium",
    "high" to "High",
    "xhigh" to "XHigh",
    "max" to "Max",
    "ultra" to "Ultra",
)

/**
 * Effort, access, Plan/Build and worktree for a chat.
 *
 * Which efforts exist depends on the assistant, so the list comes from the
 * daemon's catalog rather than being assumed here.
 */
@Composable
internal fun ChatOptionsDialog(
    model: ChatViewModel,
    state: ChatState,
    onDismiss: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val busy by model.selectingModel.collectAsStateWithLifecycle()

    LaunchedEffect(Unit) { model.loadCatalog() }

    val efforts = catalog.firstOrNull { it.id == state.backend }?.efforts.orEmpty()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Options") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                // Plan and Build are one setting: Build is Plan off.
                Section("Mode")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip(
                        selected = !state.plan,
                        onClick = { model.setPlan(false) },
                        label = { Text("Build") },
                        enabled = !busy,
                    )
                    FilterChip(
                        selected = state.plan,
                        onClick = { model.setPlan(true) },
                        label = { Text("Plan") },
                        enabled = !busy,
                    )
                }

                if (efforts.isNotEmpty()) {
                    Section("Effort")
                    Row(
                        modifier = Modifier.horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        efforts.forEach { effort ->
                            FilterChip(
                                selected = state.effort == effort,
                                onClick = { model.setEffort(effort) },
                                label = { Text(EFFORT_LABELS[effort] ?: effort) },
                                enabled = !busy,
                            )
                        }
                    }
                }

                Section("Access")
                Row(
                    modifier = Modifier.horizontalScroll(rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    ACCESS.forEach { (wire, label) ->
                        FilterChip(
                            selected = state.access == wire,
                            onClick = { model.setAccess(wire) },
                            // Planning overrides access on the daemon, so
                            // choosing one here would not mean anything.
                            enabled = !busy && !state.plan,
                            label = { Text(label) },
                        )
                    }
                }
                if (state.plan) {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        "Planning overrides access until Build is back on.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }

                Section("Worktree")
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text("New worktree")
                        Text(
                            "Run this chat in its own checkout.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.outline,
                        )
                    }
                    Switch(
                        checked = state.newWorktree,
                        onCheckedChange = { model.setNewWorktree(it) },
                        enabled = !busy,
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Close") }
        },
    )
}

@Composable
private fun Section(title: String) {
    Spacer(Modifier.height(12.dp))
    Text(
        title,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.outline,
    )
    Spacer(Modifier.height(6.dp))
}
