package com.restartfu.xd.mobile.ui

import com.restartfu.xd.protocol.GitStatusReply
import com.restartfu.xd.mobile.DiffPane
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Button
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
        state.git?.let { git -> GitSummary(git, state, model) }
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

/**
 * The checkout's branch and changes, a commit box while there is something
 * to commit, and the branch's pull request: the desktop Changes pane's header.
 */
@Composable
private fun GitSummary(git: GitStatusReply, state: DiffPane, model: DiffViewModel) {
    val signals = LocalXdSignals.current
    val uriHandler = LocalUriHandler.current
    var message by rememberSaveable(state.commits) { mutableStateOf("") }
    Column(
        Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(
                git.branch.ifBlank { "No branch" },
                modifier = Modifier.weight(1f, fill = false),
                fontFamily = XdMonoFamily,
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (git.ahead > 0) Text("↑${git.ahead}", color = signals.faint, style = MaterialTheme.typography.labelMedium)
            if (git.behind > 0) Text("↓${git.behind}", color = signals.faint, style = MaterialTheme.typography.labelMedium)
            Text(
                when {
                    git.conflicted > 0 -> "${git.conflicted} in conflict"
                    git.clean -> "No uncommitted changes"
                    else -> "${git.changed} changed"
                },
                color = if (git.conflicted > 0) MaterialTheme.colorScheme.error else signals.faint,
                style = MaterialTheme.typography.labelMedium,
            )
            Box(Modifier.weight(1f))
            state.pullRequestUrl?.let { url ->
                TextButton(onClick = { uriHandler.openUri(url) }) {
                    Text(pullRequestLabel(url))
                }
            }
        }
        if (git.canCommit) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = message,
                    onValueChange = { message = it },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text("Commit message") },
                    singleLine = true,
                    enabled = !state.committing,
                )
                Button(
                    onClick = { model.commit(message) },
                    enabled = !state.committing && message.isNotBlank(),
                ) { Text(if (state.committing) "Committing…" else "Commit all") }
            }
        }
        state.commitError?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** "#123" for a GitHub pull request address, and a plain label otherwise. */
internal fun pullRequestLabel(url: String): String =
    url.trimEnd('/').substringAfterLast('/').toLongOrNull()?.let { "#$it" } ?: "Pull request"

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
            color = MaterialTheme.colorScheme.onSurface,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text("+${file.additions}", color = LocalXdSignals.current.working)
        Text("−${file.deletions}", color = MaterialTheme.colorScheme.error)
    }
    if (!collapsed) DiffText(file.lines, Modifier.fillMaxWidth())
    HorizontalDivider()
}

@Composable
internal fun Centered(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}
