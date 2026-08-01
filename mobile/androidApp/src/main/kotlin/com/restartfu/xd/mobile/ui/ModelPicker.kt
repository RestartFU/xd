package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel

/**
 * Chooses the assistant and model for a chat.
 *
 * The list comes from the daemon rather than the app, so a daemon that gains
 * a model does not need a new build of this client to reach it.
 */
@Composable
internal fun ModelPicker(
    model: ChatViewModel,
    currentBackend: String,
    currentModel: String?,
    onDismiss: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val loading by model.catalogLoading.collectAsStateWithLifecycle()
    val error by model.catalogError.collectAsStateWithLifecycle()

    LaunchedEffect(Unit) { model.loadCatalog() }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Assistant and model") },
        text = {
            when {
                loading && catalog.isEmpty() -> Box(
                    Modifier.fillMaxWidth(),
                    contentAlignment = Alignment.Center,
                ) { CircularProgressIndicator() }

                error != null && catalog.isEmpty() -> Text(
                    error.orEmpty(),
                    color = MaterialTheme.colorScheme.error,
                )

                else -> LazyColumn(Modifier.heightIn(max = 420.dp)) {
                    catalog.forEach { backend ->
                        item(key = "backend-${backend.id}") {
                            Row(
                                modifier = Modifier.padding(top = 12.dp, bottom = 4.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                BackendIcon(backend.id, size = 16.dp)
                                Text(
                                    backend.name,
                                    style = MaterialTheme.typography.labelLarge,
                                    color = MaterialTheme.colorScheme.outline,
                                )
                            }
                            HorizontalDivider()
                        }
                        items(backend.models, key = { "${backend.id}-${it.id}" }) { entry ->
                            // A chat's model only means something alongside its
                            // assistant: the same name could exist under both.
                            val selected = backend.id == currentBackend &&
                                entry.id == (currentModel ?: backend.defaultModel)
                            Row(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        onDismiss()
                                        if (!selected) {
                                            model.selectModel(backend.id, entry.id)
                                        }
                                    }
                                    .padding(vertical = 12.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(if (selected) "✓" else " ", Modifier.width(24.dp))
                                Column(Modifier.weight(1f)) {
                                    Text(
                                        entry.name,
                                        maxLines = 1,
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
                        item(key = "gap-${backend.id}") { Spacer(Modifier.width(1.dp)) }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Close") }
        },
    )
}
