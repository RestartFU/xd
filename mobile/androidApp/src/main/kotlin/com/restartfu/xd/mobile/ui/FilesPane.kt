package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.files.FileTab
import com.restartfu.xd.files.TreeRow
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.syntax.Syntax

/** How far one level of nesting shifts a row. */
private val INDENT = 12.dp

/**
 * The working directory as a folding tree, and the files opened out of it.
 *
 * A tree rather than one directory at a time: the shape of a repository is
 * most of what tells you where you are in it, and a browser that replaces its
 * contents on every tap hides exactly that.
 */
@Composable
internal fun FilesPaneContent(model: FilesViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val rows = state.tree.rows()

    Column(Modifier.fillMaxSize()) {
        FileTabs(
            open = state.open,
            onShow = model::show,
            onClose = model::close,
            trailing = {
                TextButton(onClick = model::refresh, enabled = !state.loading) { Text("Refresh") }
            },
        )

        state.error?.let { message ->
            Text(
                message,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }

        when (val tab = state.open.active) {
            FileTab.Tree -> when {
                rows.isEmpty() && state.loading -> Centered { CircularProgressIndicator() }
                rows.isEmpty() -> Centered {
                    Text("Nothing here", style = MaterialTheme.typography.bodySmall)
                }
                else -> LazyColumn(Modifier.fillMaxSize()) {
                    items(rows, key = { it.path }) { row ->
                        TreeEntry(
                            row = row,
                            onClick = {
                                if (row.directory) model.toggle(row.path) else model.open(row.path)
                            },
                        )
                    }
                }
            }
            is FileTab.File -> {
                val file = state.open.current
                if (file == null) {
                    Centered { CircularProgressIndicator() }
                } else {
                    FileEditor(
                        text = file.text,
                        path = tab.path,
                        dirty = file.dirty,
                        saving = file.saving,
                        error = file.error,
                        onChange = { model.edit(tab.path, it) },
                        onSave = { model.save(tab.path) },
                    )
                }
            }
        }
    }
}

/**
 * The tree, then a tab per open file.
 *
 * Scrolls sideways rather than wrapping: on a phone even three paths will not
 * fit, and a strip that grew taller would take the room the file needs.
 */
@Composable
private fun FileTabs(
    open: com.restartfu.xd.files.OpenFiles,
    onShow: (FileTab) -> Unit,
    onClose: (String) -> Unit,
    trailing: @Composable () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            modifier = Modifier
                .weight(1f)
                .horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            TabLabel(
                text = "Files",
                selected = open.active == FileTab.Tree,
                onClick = { onShow(FileTab.Tree) },
            )
            open.files.forEach { file ->
                TabLabel(
                    // A dot rather than a word: the tab is already as narrow as
                    // it can be and still name the file.
                    text = if (file.dirty) "${file.name} •" else file.name,
                    selected = open.active == FileTab.File(file.path),
                    onClick = { onShow(FileTab.File(file.path)) },
                    onClose = { onClose(file.path) },
                )
            }
        }
        trailing()
    }
}

@Composable
private fun TabLabel(
    text: String,
    selected: Boolean,
    onClick: () -> Unit,
    onClose: (() -> Unit)? = null,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(
            text,
            modifier = Modifier
                .clickable(onClick = onClick)
                .padding(vertical = 8.dp, horizontal = 4.dp),
            maxLines = 1,
            style = MaterialTheme.typography.bodySmall,
            fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal,
            color = if (selected) {
                MaterialTheme.colorScheme.primary
            } else {
                MaterialTheme.colorScheme.onSurfaceVariant
            },
        )
        onClose?.let { close ->
            Text(
                "✕",
                modifier = Modifier
                    .clickable(onClick = close)
                    .padding(vertical = 8.dp, horizontal = 2.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
        }
    }
}

@Composable
private fun TreeEntry(row: TreeRow, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .padding(start = 8.dp + INDENT * row.depth, end = 12.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // A caret only where there is something to fold, and a blank of the
        // same width elsewhere so names line up down a level.
        Box(Modifier.width(16.dp), contentAlignment = Alignment.Center) {
            when {
                row.loading -> CircularProgressIndicator(Modifier.size(10.dp), strokeWidth = 1.dp)
                row.directory -> Text(
                    if (row.expanded) "▾" else "▸",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.outline,
                )
            }
        }
        Spacer(Modifier.width(4.dp))
        Text(
            row.name,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodySmall,
            fontFamily = if (row.directory) null else FontFamily.Monospace,
            color = if (row.directory) {
                MaterialTheme.colorScheme.onSurface
            } else {
                MaterialTheme.colorScheme.onSurfaceVariant
            },
        )
    }
}

@Composable
private fun FileEditor(
    text: String,
    path: String,
    dirty: Boolean,
    saving: Boolean,
    error: String?,
    onChange: (String) -> Unit,
    onSave: () -> Unit,
) {
    val language = remember(path) { Syntax.languageForPath(path) }
    val fallback = LocalContentColor.current
    val syntaxColours = remember(language, fallback) {
        SyntaxVisualTransformation(language, fallback)
    }

    Column(Modifier.fillMaxSize()) {
        error?.let { message ->
            Text(
                message,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        OutlinedTextField(
            value = text,
            onValueChange = onChange,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 8.dp),
            textStyle = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            visualTransformation = syntaxColours,
        )
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.End,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TextButton(onClick = onSave, enabled = dirty && !saving) {
                Text(if (saving) "Saving…" else "Save")
            }
        }
    }
}
