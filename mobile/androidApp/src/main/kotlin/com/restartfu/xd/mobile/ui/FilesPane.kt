package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.protocol.FileEntryReply
import com.restartfu.xd.syntax.Syntax

@Composable
internal fun FilesPaneContent(model: FilesViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val atRoot = state.path.isEmpty() && state.preview == null

    Column(Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton(onClick = model::up, enabled = !atRoot) { Text("Up") }
            Spacer(Modifier.width(8.dp))
            Text(
                state.preview?.let { state.previewPath }
                    ?: state.path.ifEmpty { "working directory" },
                modifier = Modifier.weight(1f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodySmall,
            )
            TextButton(onClick = model::refresh, enabled = !state.loading) { Text("Refresh") }
        }

        when {
            state.loading && state.entries.isEmpty() && state.preview == null ->
                Centered { CircularProgressIndicator() }

            state.error != null -> Centered {
                Text(
                    state.error.orEmpty(),
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(16.dp),
                )
            }

            state.preview != null -> {
                val language = remember(state.previewPath) {
                    Syntax.languageForPath(state.previewPath)
                }
                CodeText(
                    state.preview.orEmpty(),
                    language,
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(12.dp),
                )
            }

            state.entries.isEmpty() -> Centered {
                Text("Empty directory", color = MaterialTheme.colorScheme.outline)
            }

            else -> LazyColumn(Modifier.fillMaxSize()) {
                items(state.entries, key = FileEntryReply::name) { entry ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { model.enter(entry) }
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(if (entry.directory) "▸" else " ")
                        Text(
                            entry.name,
                            modifier = Modifier.weight(1f),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        }
        Box(Modifier.fillMaxWidth())
    }
}
