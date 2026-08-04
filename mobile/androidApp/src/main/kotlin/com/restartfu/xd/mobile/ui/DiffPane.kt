package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.syntax.CodeBlocks
import com.restartfu.xd.syntax.DiffFile

@Composable
internal fun DiffPaneContent(model: DiffViewModel) {
    val state by model.state.collectAsStateWithLifecycle()

    Column(Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            FilterChip(
                selected = !state.branch,
                onClick = { model.showBranch(false) },
                label = { Text("Working") },
            )
            FilterChip(
                selected = state.branch,
                onClick = { model.showBranch(true) },
                label = { Text("Branch") },
            )
            Box(Modifier.weight(1f))
            TextButton(onClick = model::refresh, enabled = !state.loading) {
                Text("Refresh")
            }
        }

        when {
            state.loading && state.patch.isEmpty() -> Centered { CircularProgressIndicator() }
            state.error != null -> Centered {
                Text(
                    state.error.orEmpty(),
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(16.dp),
                )
            }
            state.patch.isBlank() -> Centered {
                Text(
                    if (state.branch) "No branch changes" else "Nothing changed",
                    color = MaterialTheme.colorScheme.outline,
                )
            }
            else -> {
                val files = remember(state.patch) { CodeBlocks.diffFiles(state.patch) }
                Column(
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(vertical = 4.dp),
                ) {
                    files.forEach { file ->
                        key(file.path) {
                            DiffFileSection(
                                file = file,
                                collapsed = file.path in state.collapsedFiles,
                                onToggle = { model.toggleFile(file.path) },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DiffFileSection(
    file: DiffFile,
    collapsed: Boolean,
    onToggle: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(role = Role.Button, onClick = onToggle)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(if (collapsed) "▸" else "▾", color = MaterialTheme.colorScheme.outline)
        Text(
            file.path,
            modifier = Modifier.weight(1f),
            color = Color(0xFFFFBE6F),
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text("+${file.additions}", color = Color(0xFF57E389))
        Text("−${file.deletions}", color = Color(0xFFF66151))
    }
    if (!collapsed) DiffText(file.lines, Modifier.fillMaxWidth())
    HorizontalDivider()
}

@Composable
internal fun Centered(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}
