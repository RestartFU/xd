package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
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
    val auth by model.agentAuth.collectAsStateWithLifecycle()
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val now = rememberMinuteClock()

    LaunchedEffect(Unit) { model.loadCatalog() }
    LaunchedEffect(tree.chatAccounts) { model.loadAccounts() }
    val accounts = if (tree.chatAccounts) chatAccountChoices(auth?.get(state.backend)) else emptyList()
    // An account removed since it was chosen chooses nothing.
    val chosen = state.account?.takeIf { id -> accounts.any { it.id == id } }

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
                Row(Modifier.selectableGroup(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ChoiceChip(
                        selected = !state.plan,
                        onClick = { model.setPlan(false) },
                        label = { Text("Build") },
                        enabled = !busy,
                    )
                    ChoiceChip(
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
                        modifier = Modifier.fillMaxWidth().selectableGroup(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        efforts.forEach { effort ->
                            ChoiceChip(
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
                    modifier = Modifier.fillMaxWidth().selectableGroup(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    ACCESS.forEach { wire ->
                        ChoiceChip(
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
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                if (accounts.isNotEmpty()) {
                    Section("Account")
                    Column(Modifier.selectableGroup()) {
                        AccountChoice(
                            label = AUTOMATIC_ACCOUNT,
                            selected = chosen == null,
                            enabled = !busy,
                            onClick = { model.setAccount(null) },
                        )
                        accounts.forEach { account ->
                            AccountChoice(
                                label = chatAccountLabel(account, now),
                                selected = chosen == account.id,
                                enabled = !busy,
                                onClick = { model.setAccount(account.id) },
                            )
                        }
                    }
                }

                if (state.canCreateWorktree) {
                    Section("Worktree")
                    OptionSwitch(
                        title = "New worktree",
                        description = "Run this session in its own checkout.",
                        checked = state.newWorktree,
                        enabled = !busy,
                        onCheckedChange = { model.setNewWorktree(it) },
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
private fun OptionSwitch(
    title: String,
    description: String,
    checked: Boolean,
    enabled: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    // One stop, named by its words: a tap anywhere on the row turns it.
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = onCheckedChange),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(title)
            Text(
                description,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Switch(checked = checked, onCheckedChange = null, enabled = enabled)
    }
}

@Composable
internal fun AccountChoice(
    label: String,
    selected: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .selectable(selected = selected, enabled = enabled, role = Role.RadioButton, onClick = onClick)
            .padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        RadioButton(selected = selected, onClick = null, enabled = enabled)
        Text(label, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun Section(title: String) {
    Spacer(Modifier.height(12.dp))
    Text(
        title,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(6.dp))
}
