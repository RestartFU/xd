package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import com.restartfu.xd.model.DisplayLabels

/** The access levels the desktop offers. Plan is the separate toggle. */
private val ACCESS = listOf(
    "read-only", "edit", "full",
)

/**
 * Effort, access, Plan/Build and worktree for a chat.
 *
 * Which efforts exist depends on the assistant, so the list comes from the
 * host's catalog rather than being assumed here.
 */
@Composable
@OptIn(ExperimentalLayoutApi::class)
internal fun ChatOptionsDialog(
    model: ChatViewModel,
    state: ChatState,
    onDismiss: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val busy by model.selectingModel.collectAsStateWithLifecycle()

    LaunchedEffect(Unit) { model.loadCatalog() }

    val efforts = catalog.firstOrNull { it.id == state.backend }
        ?.efforts
        .orEmpty()

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Chat settings") },
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

                if (state.backend == "codex") {
                    Section("Codex")
                    OptionSwitch(
                        title = "Fast mode",
                        description = "Use priority service. May consume usage credits faster.",
                        checked = state.fast,
                        enabled = !busy,
                        onCheckedChange = model::setFast,
                    )
                }

                if (efforts.isNotEmpty()) {
                    Section("Reasoning effort")
                    FlowRow(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        efforts.forEach { effort ->
                            FilterChip(
                                selected = state.effort == effort,
                                onClick = { model.setEffort(effort) },
                                label = { Text(DisplayLabels.effort(effort)) },
                                enabled = !busy,
                            )
                        }
                    }
                }

                Section("Access")
                FlowRow(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    ACCESS.forEach { wire ->
                        FilterChip(
                            selected = DisplayLabels.canonicalAccess(state.access) == wire,
                            onClick = { model.setAccess(wire) },
                            // Planning overrides access on the host, so
                            // choosing one here would not mean anything.
                            enabled = !busy && !state.plan,
                            label = { Text(DisplayLabels.access(wire)) },
                        )
                    }
                }
                if (state.plan) {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        "Switch to Build to change access.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }

                if (state.canCreateWorktree) {
                    Section("Worktree")
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text("New worktree")
                            Text(
                                "Run this session in its own checkout.",
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
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Close") }
        },
    )
}

@Composable
private fun OptionSwitch(
    title: String,
    description: String,
    checked: Boolean,
    enabled: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(title)
            Text(
                description,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
        }
        Switch(
            checked = checked,
            onCheckedChange = onCheckedChange,
            enabled = enabled,
        )
    }
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
