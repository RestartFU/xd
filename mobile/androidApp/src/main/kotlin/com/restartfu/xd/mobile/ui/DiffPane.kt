package com.restartfu.xd.mobile.ui

import com.restartfu.xd.protocol.GitStatusReply
import com.restartfu.xd.mobile.DiffPane
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.runtime.setValue
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.ScrollableState
import androidx.compose.foundation.gestures.scrollable
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
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
        state.gitError?.let { error ->
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    if (state.git == null) "Git status could not be read: $error"
                    else "This may be out of date: Git status could not be read again ($error)",
                    modifier = Modifier.weight(1f),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
                TextButton(onClick = model::refresh) { Text("Try again") }
            }
        }
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
            else -> PatchList(state.patch, state.collapsedFiles, model::toggleFile)
        }
    }
}

/** A patch's files, parsed and coloured away from the main thread. */
private class PreparedDiff(val patch: String, val files: List<PreparedFile>)

private class PreparedFile(val file: DiffFile, val lines: List<AnnotatedString>)

/**
 * Every file of a patch, each line its own row of a lazy list.
 *
 * A branch's patch can run to megabytes, so only the lines on screen are
 * laid out, and parsing and colouring happen in the background. Until the
 * colours are ready the lines show uncoloured.
 */
@Composable
private fun PatchList(
    patch: String,
    collapsedFiles: Set<String>,
    onToggle: (String) -> Unit,
) {
    val fallback = LocalContentColor.current
    // Kept across a refresh, so the old patch stays up until the new one is ready.
    val parsed by produceState<PreparedDiff?>(null, patch) {
        val files = withContext(Dispatchers.Default) { CodeBlocks.diffFiles(patch) }
        value = PreparedDiff(patch, files.map { PreparedFile(it, it.lines.map(::plainDiffLine)) })
    }
    // Paired with the parse it colours, so a newer parse is never shown in older colours.
    val coloured by produceState<Pair<PreparedDiff, PreparedDiff>?>(null, parsed, fallback) {
        val plain = parsed ?: return@produceState
        value = plain to withContext(Dispatchers.Default) {
            PreparedDiff(plain.patch, plain.files.map { PreparedFile(it.file, highlightDiff(it.file.lines, fallback)) })
        }
    }
    val diff = coloured?.takeIf { it.first === parsed }?.second ?: parsed
    if (diff == null) {
        Centered { CircularProgressIndicator() }
        return
    }
    // Each file's lines pan sideways together, as one block did before.
    val pans = remember(diff.patch) { HashMap<String, LinePan>() }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 4.dp)) {
        diff.files.forEachIndexed { index, prepared ->
            val file = prepared.file
            val collapsed = file.path in collapsedFiles
            item(key = "file-$index-${file.path}", contentType = "file") {
                DiffFileHeader(file, collapsed) { onToggle(file.path) }
            }
            if (!collapsed) {
                val pan = pans.getOrPut("$index-${file.path}") { LinePan() }
                items(
                    count = prepared.lines.size,
                    key = { line -> "line-$index-${file.path}-$line" },
                    contentType = { "line" },
                ) { line ->
                    PannedLine(pan) { lineModifier ->
                        DiffLineText(file.lines[line].kind, prepared.lines[line], lineModifier)
                    }
                }
            }
            item(key = "divider-$index-${file.path}", contentType = "divider") { HorizontalDivider() }
        }
    }
}

/**
 * How far one file's lines are panned sideways. Lines are laid out only while
 * on screen, so the widest is the widest seen so far.
 */
private class LinePan {
    var offset by mutableFloatStateOf(0f)
    var widest by mutableIntStateOf(0)
    var viewport by mutableIntStateOf(0)

    val state = ScrollableState { delta ->
        val limit = (widest - viewport).coerceAtLeast(0).toFloat()
        val next = (offset - delta).coerceIn(0f, limit)
        val consumed = offset - next
        offset = next
        consumed
    }
}

/** A line drawn at its full width, panned by [pan] and clipped to the screen. */
@Composable
private fun PannedLine(pan: LinePan, line: @Composable (Modifier) -> Unit) {
    Box(
        Modifier
            .fillMaxWidth()
            .clipToBounds()
            .onSizeChanged { pan.viewport = it.width }
            .scrollable(pan.state, Orientation.Horizontal),
    ) {
        line(
            Modifier
                .wrapContentWidth(Alignment.Start, unbounded = true)
                .offset { IntOffset(-pan.offset.roundToInt(), 0) }
                .onSizeChanged { if (it.width > pan.widest) pan.widest = it.width },
        )
    }
}

/**
 * The checkout's branch and changes, a commit box while there is something
 * to commit, and the branch's pull request: the desktop Changes pane's header.
 */
@Composable
private fun GitSummary(git: GitStatusReply, state: DiffPane, model: DiffViewModel) {
    val signals = LocalXdSignals.current
    val openLink = rememberLinkOpener()
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
                TextButton(onClick = { openLink(url) }) {
                    Text(pullRequestLabel(url))
                }
            }
        }
        if (git.canCommit) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = state.commitMessage,
                    onValueChange = model::editCommitMessage,
                    modifier = Modifier.weight(1f),
                    placeholder = { Text("Commit message") },
                    singleLine = true,
                    enabled = !state.committing,
                )
                Button(
                    onClick = model::commit,
                    enabled = !state.committing && state.commitMessage.isNotBlank(),
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
private fun DiffFileHeader(
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
}

@Composable
internal fun Centered(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
}
