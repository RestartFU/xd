package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.DisplayLabels

/**
 * Chooses a model for the assistant fixed to this chat.
 *
 * The list comes from the host rather than the app, so a host that gains
 * a model does not need a new build of this client to reach it.
 */
@Composable
internal fun ModelPicker(
    model: ChatViewModel,
    currentBackend: String,
    currentModel: String?,
    currentEffort: String?,
    onDismiss: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val loading by model.catalogLoading.collectAsStateWithLifecycle()
    val error by model.catalogError.collectAsStateWithLifecycle()
    val busy by model.selectingModel.collectAsStateWithLifecycle()
    val selectionError by model.modelSelectionError.collectAsStateWithLifecycle()
    var effortBackendId by remember { mutableStateOf<String?>(null) }
    var effortModelId by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        model.clearModelSelectionError()
        model.loadCatalog()
    }

    val currentAssistant = catalog.firstOrNull { it.id == currentBackend }
    val effortBackend = catalog.firstOrNull { it.id == effortBackendId }
    val efforts = effortBackend?.efforts.orEmpty()
    val modelReady = effortBackend != null &&
        currentBackend == effortBackend.id &&
        (currentModel ?: effortBackend.defaultModel) == effortModelId

    LaunchedEffect(modelReady, efforts) {
        if (modelReady && efforts.isEmpty()) onDismiss()
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(if (effortBackendId == null) "Choose model" else "Reasoning effort")
        },
        text = {
            when {
                effortBackendId != null && !modelReady && selectionError != null -> Column(
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Text(selectionError.orEmpty(), color = MaterialTheme.colorScheme.error)
                    TextButton(onClick = { effortModelId?.let(model::selectModel) }) { Text("Retry") }
                }

                effortBackendId != null && !modelReady -> Box(
                    Modifier.fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                effortBackend != null -> LazyColumn(Modifier.heightIn(max = 420.dp)) {
                    items(efforts, key = { it }) { effort ->
                        val selected = effort == currentEffort
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(min = 48.dp)
                                .selectable(selected = selected, enabled = !busy, role = Role.RadioButton) {
                                    if (!selected) model.setEffort(effort)
                                    onDismiss()
                                }
                                .padding(vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            SelectionMark(selected)
                            Text(
                                DisplayLabels.effort(effort),
                                fontWeight = if (selected) FontWeight.Bold else null,
                            )
                        }
                    }
                }

                loading && catalog.isEmpty() -> Box(
                    Modifier.fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                error != null && catalog.isEmpty() -> Column(
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Text(error.orEmpty(), color = MaterialTheme.colorScheme.error)
                    TextButton(onClick = model::loadCatalog) { Text("Retry") }
                }

                currentAssistant == null -> Text(
                    "No models are available for this assistant.",
                    color = MaterialTheme.colorScheme.error,
                )

                else -> LazyColumn(Modifier.heightIn(max = 420.dp)) {
                    item(key = "backend-${currentAssistant.id}") {
                        Row(
                            modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            BackendIcon(currentAssistant.id, size = 16.dp)
                            Text(
                                DisplayLabels.backend(currentAssistant.id, catalog),
                                style = MaterialTheme.typography.labelLarge,
                                color = MaterialTheme.colorScheme.outline,
                            )
                        }
                        HorizontalDivider()
                    }
                    items(
                        currentAssistant.models,
                        key = { "${currentAssistant.id}-${it.id}" },
                    ) { entry ->
                        val selected =
                            entry.id == (currentModel ?: currentAssistant.defaultModel)
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(min = 48.dp)
                                .selectable(selected = selected, enabled = !busy, role = Role.RadioButton) {
                                    model.clearModelSelectionError()
                                    if (!selected) model.selectModel(entry.id)
                                    effortBackendId = currentAssistant.id
                                    effortModelId = entry.id
                                }
                                .padding(vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            SelectionMark(selected)
                            Column(Modifier.weight(1f)) {
                                Text(
                                    DisplayLabels.model(entry.id, currentAssistant.id, catalog),
                                    maxLines = 2,
                                    overflow = TextOverflow.Ellipsis,
                                    fontWeight = if (selected) FontWeight.Bold else null,
                                )
                                if (entry.contextWindow > 0) {
                                    Text(
                                        "${entry.contextWindow / 1000}k context",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.outline,
                                    )
                                }
                            }
                        }
                    }
                    item(key = "gap-${currentAssistant.id}") { Spacer(Modifier.width(1.dp)) }
                }
            }
        },
        confirmButton = {
            if (effortBackendId == null) {
                TextButton(onClick = onDismiss) { Text("Close") }
            } else {
                TextButton(
                    onClick = {
                        model.clearModelSelectionError()
                        effortBackendId = null
                        effortModelId = null
                    },
                ) { Text("Back") }
            }
        },
        dismissButton = if (effortBackendId == null) null else {
            { TextButton(onClick = onDismiss) { Text("Cancel") } }
        },
    )
}

@Composable
private fun SelectionMark(selected: Boolean) {
    Box(Modifier.width(24.dp), contentAlignment = Alignment.CenterStart) {
        if (selected) Icon(
            painterResource(R.drawable.ic_check),
            contentDescription = null,
            modifier = Modifier.size(18.dp),
            tint = MaterialTheme.colorScheme.primary,
        )
    }
}
