package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.protocol.DirectoryListReply

/**
 * Browses folders on the connected host, so a project's repository can be
 * picked rather than typed out on a phone keyboard.
 */
@Composable
internal fun RepositoryPickerDialog(
    listDirectories: suspend (String?) -> DirectoryListReply,
    initialPath: String?,
    onDismiss: () -> Unit,
    onChoose: (String) -> Unit,
) {
    var requestedPath by rememberSaveable { mutableStateOf(initialPath?.takeIf(String::isNotBlank)) }
    var currentPath by rememberSaveable { mutableStateOf("") }
    var entries by remember { mutableStateOf(emptyList<String>()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableStateOf(0) }

    LaunchedEffect(requestedPath, attempt) {
        loading = true
        error = null
        try {
            val result = listDirectories(requestedPath)
            currentPath = result.path
            entries = result.entries
        } catch (failure: kotlinx.coroutines.CancellationException) {
            throw failure
        } catch (failure: Throwable) {
            error = failure.message ?: "Could not list that folder"
            entries = emptyList()
            // Show where it failed, so Up leads to that folder's parent rather
            // than asking for the same folder again.
            requestedPath?.let { currentPath = it }
        } finally {
            loading = false
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Choose repository") },
        text = {
            Column {
                Row(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        currentPath.ifBlank { "Loading…" },
                        modifier = Modifier.weight(1f).padding(top = 12.dp),
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        fontFamily = XdMonoFamily,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    parentDirectory(currentPath)?.let { parent ->
                        TextButton(
                            onClick = { requestedPath = parent },
                            enabled = !loading,
                        ) { Text("Up") }
                    }
                }
                error?.let {
                    Text(
                        it,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Row {
                        TextButton(onClick = { attempt++ }, enabled = !loading) { Text("Try again") }
                        // A typed path that is wrong would otherwise leave nowhere to go.
                        if (requestedPath != null) {
                            TextButton(onClick = { requestedPath = null }, enabled = !loading) { Text("Home folder") }
                        }
                    }
                }
                when {
                    loading -> CircularProgressIndicator(Modifier.padding(vertical = 20.dp))
                    error != null -> Unit
                    entries.isEmpty() -> Text(
                        "No folders inside this one.",
                        modifier = Modifier.padding(vertical = 20.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> LazyColumn(Modifier.heightIn(max = 320.dp)) {
                        items(entries, key = { it }) { entry ->
                            Text(
                                entry,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .heightIn(min = 48.dp)
                                    .clickable { requestedPath = joinDirectory(currentPath, entry) }
                                    .padding(vertical = 14.dp),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onChoose(currentPath) },
                enabled = !loading && error == null && currentPath.isNotBlank(),
            ) { Text("Use this folder") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

// The host runs on Linux, macOS, or WSL, so paths are Unix paths: `/` is the
// only separator, and a backslash is an ordinary character in a name.
internal fun joinDirectory(parent: String, child: String): String =
    if (parent.endsWith('/')) parent + child else "$parent/$child"

internal fun parentDirectory(path: String): String? {
    val trimmed = path.trimEnd('/')
    if (trimmed.isEmpty()) return null
    val separator = trimmed.lastIndexOf('/')
    return when {
        separator < 0 -> null
        separator == 0 -> "/"
        else -> trimmed.substring(0, separator)
    }
}
