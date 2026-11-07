package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.ShortcutEditorState

@Composable
internal fun ShortcutEditorDialog(
    state: ShortcutEditorState,
    onDismiss: () -> Unit,
    onSave: (List<String>) -> Unit,
    onRetry: () -> Unit,
) {
    // Saved, so recreating the activity keeps unsaved edits; keyed on the
    // opening and the loaded list, so a fresh load starts from the host's.
    var prompts by rememberSaveable(state.opening, state.prompts, stateSaver = PromptsSaver) {
        mutableStateOf(state.prompts)
    }
    // Locked while saving: the request carries the list as Save found it.
    val editable = !state.saving

    AlertDialog(
        onDismissRequest = { if (!state.saving) onDismiss() },
        title = { Text(state.title) },
        text = {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    if (state.folderId == null) {
                        "These prompt buttons appear in every project on this host."
                    } else {
                        "These prompt buttons appear in this project, after the ones every project shares."
                    },
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                if (state.loading) {
                    CircularProgressIndicator(Modifier.align(Alignment.CenterHorizontally))
                } else {
                    state.error?.let { message ->
                        Text(message, color = MaterialTheme.colorScheme.error)
                    }
                    if (!state.loaded) {
                        TextButton(onClick = onRetry) { Text("Try again") }
                        return@Column
                    }
                    prompts.forEachIndexed { index, prompt ->
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            OutlinedTextField(
                                value = prompt,
                                onValueChange = { value ->
                                    prompts = prompts.toMutableList().also { it[index] = value }
                                },
                                modifier = Modifier.weight(1f),
                                label = { Text("Prompt") },
                                minLines = 1,
                                maxLines = 4,
                                enabled = editable,
                            )
                            TextButton(
                                onClick = {
                                    prompts = prompts.filterIndexed { at, _ -> at != index }
                                },
                                enabled = editable,
                            ) { Text("Remove") }
                        }
                    }
                    TextButton(
                        onClick = { prompts = prompts + "" },
                        enabled = editable && prompts.size < 24,
                    ) { Text("Add prompt") }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    onSave(prompts.map { it.trim() }.filter { it.isNotEmpty() })
                },
                enabled = state.loaded && !state.saving,
            ) { Text(if (state.saving) "Saving…" else "Save") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss, enabled = !state.saving) { Text("Cancel") }
        },
    )
}

private val PromptsSaver = listSaver<List<String>, String>(save = { it }, restore = { it })
