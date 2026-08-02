package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.net.Link

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TreeScreen(
    model: MainViewModel,
    link: Link,
    openChat: (String) -> Unit,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()
    val createdChat by model.createdChat.collectAsStateWithLifecycle()
    var expandedIds by rememberSaveable { mutableStateOf(emptyList<String>()) }
    val roots = tree.folders.filter { it.parentId == null }
    val children = tree.folders.groupBy(Folder::parentId)
    val chats = tree.chats.groupBy(ChatSummary::folderId)
    val foldersById = tree.folders.associateBy(Folder::id)
    var choosingFolder by rememberSaveable { mutableStateOf(false) }
    var updatingDaemon by rememberSaveable { mutableStateOf(false) }
    var acting by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var renaming by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var deleting by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var confirmingForget by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(createdChat) {
        createdChat?.let { chatId ->
            openChat(chatId)
            model.consumeCreatedChat(chatId)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("xd") },
                actions = {
                    TextButton(onClick = { updatingDaemon = true }) {
                        Text("Update")
                    }
                    TextButton(onClick = { confirmingForget = true }) {
                        Text("Forget")
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton(
                onClick = { choosingFolder = true },
            ) { Text("New") }
        },
    ) { padding ->
        Column(Modifier.padding(padding)) {
            ConnectionBanner(link, model.client::poke)
            if (tree.loading && tree.folders.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
            } else {
                (tree.error ?: operationError)?.let {
                    Text(
                        it,
                        modifier = Modifier.padding(16.dp),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                LazyColumn(contentPadding = PaddingValues(bottom = 88.dp)) {
                    roots.forEach { folder ->
                        folderRows(
                            folder = folder,
                            depth = 0,
                            children = children,
                            chats = chats,
                            expanded = expandedIds.toSet(),
                            toggle = { id ->
                                expandedIds = if (id in expandedIds) {
                                    expandedIds - id
                                } else {
                                    expandedIds + id
                                }
                            },
                            openChat = openChat,
                            actOnChat = { chat -> acting = chat.id to chat.title },
                        )
                    }
                }
            }
        }
    }
    acting?.let { (chatId, title) ->
        ChatActionsDialog(
            title = title,
            onDismiss = { acting = null },
            onRename = {
                acting = null
                renaming = chatId to title
            },
            onDelete = {
                acting = null
                deleting = chatId to title
            },
        )
    }
    renaming?.let { (chatId, title) ->
        RenameChatDialog(
            title = title,
            onDismiss = { renaming = null },
            onRename = { name ->
                renaming = null
                if (name != title) model.renameChat(chatId, name)
            },
        )
    }
    deleting?.let { (chatId, title) ->
        AlertDialog(
            onDismissRequest = { deleting = null },
            title = { Text("Delete this chat?") },
            text = {
                val name = title.ifBlank { "This chat" }
                Text(
                    name + " and its messages will be removed on the daemon. " +
                        "This cannot be undone.",
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        deleting = null
                        model.deleteChat(chatId)
                    },
                ) { Text("Delete") }
            },
            dismissButton = {
                TextButton(onClick = { deleting = null }) { Text("Cancel") }
            },
        )
    }
    if (updatingDaemon) {
        DaemonUpdateDialog(model) { updatingDaemon = false }
    }
    if (choosingFolder) {
        AlertDialog(
            onDismissRequest = { choosingFolder = false },
            title = { Text("Choose a folder") },
            text = {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState()),
                ) {
                    if (tree.folders.isEmpty()) {
                        Text("No workspace folders are available.")
                    }
                    tree.folders.forEach { folder ->
                        Text(
                            folderPath(folder, foldersById),
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    choosingFolder = false
                                    model.createChat(folder.id)
                                }
                                .padding(vertical = 12.dp),
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { choosingFolder = false }) { Text("Cancel") }
            },
        )
    }
    if (confirmingForget) {
        AlertDialog(
            onDismissRequest = { confirmingForget = false },
            title = { Text("Forget this remote?") },
            text = {
                Text("Pairing credentials will be erased. You will need a new code to reconnect.")
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmingForget = false
                        model.forget()
                    },
                ) { Text("Forget") }
            },
            dismissButton = {
                TextButton(onClick = { confirmingForget = false }) { Text("Cancel") }
            },
        )
    }
}

/**
 * What a long press on a chat offers.
 *
 * A list rather than dialog buttons: these are two things to do, not a
 * question with a yes and a no, and a destructive action does not belong where
 * a thumb expects Confirm.
 */
@Composable
private fun ChatActionsDialog(
    title: String,
    onDismiss: () -> Unit,
    onRename: () -> Unit,
    onDelete: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                title.ifBlank { "This chat" },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        },
        text = {
            Column(Modifier.fillMaxWidth()) {
                Text(
                    "Rename",
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(onClick = onRename)
                        .padding(vertical = 12.dp),
                )
                Text(
                    "Delete",
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(onClick = onDelete)
                        .padding(vertical = 12.dp),
                    color = MaterialTheme.colorScheme.error,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

@Composable
private fun RenameChatDialog(
    title: String,
    onDismiss: () -> Unit,
    onRename: (String) -> Unit,
) {
    // Keyed on the title so reopening on a different chat starts from its own
    // name rather than the last one edited.
    var name by rememberSaveable(title) { mutableStateOf(title) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rename chat") },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                modifier = Modifier.fillMaxWidth(),
                label = { Text("Title") },
                singleLine = true,
            )
        },
        confirmButton = {
            TextButton(
                // The daemon refuses a blank title rather than clearing it, so
                // there is nothing to send.
                onClick = { onRename(name.trim()) },
                enabled = name.isNotBlank(),
            ) { Text("Rename") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

@Composable
private fun ConnectionBanner(
    link: Link,
    retry: () -> Unit,
) {
    when (link) {
        Link.Connecting -> Text(
            "Connecting…",
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
        )
        is Link.Down -> Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Offline — retrying in ${link.nextAttemptInMs / 1000}s",
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = retry) { Text("Retry") }
        }
        else -> Unit
    }
}

/** Guards against a cycle in daemon-supplied parent ids. */
private fun folderPath(
    folder: Folder,
    foldersById: Map<String, Folder>,
): String {
    val names = mutableListOf<String>()
    val visited = mutableSetOf<String>()
    var current: Folder? = folder
    while (current != null && visited.add(current.id)) {
        names += current.name
        current = current.parentId?.let(foldersById::get)
    }
    return names.asReversed().joinToString(" / ")
}

@OptIn(ExperimentalFoundationApi::class)
private fun LazyListScope.folderRows(
    folder: Folder,
    depth: Int,
    children: Map<String?, List<Folder>>,
    chats: Map<String, List<ChatSummary>>,
    expanded: Set<String>,
    toggle: (String) -> Unit,
    openChat: (String) -> Unit,
    actOnChat: (ChatSummary) -> Unit,
) {
    item(key = "folder-${folder.id}") {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable { toggle(folder.id) }
                .padding(start = (16 + depth * 16).dp, top = 12.dp, bottom = 12.dp),
        ) {
            Text(if (folder.id in expanded) "▾" else "▸")
            Spacer(Modifier.width(8.dp))
            Text(
                folder.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
    if (folder.id in expanded) {
        chats[folder.id].orEmpty().forEach { chat ->
            item(key = "chat-${chat.id}") {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        // Long press rather than visible buttons: renaming and
                        // deleting are both rare, and the row is already a tap
                        // target for the thing you usually want.
                        .combinedClickable(
                            onClick = { openChat(chat.id) },
                            onLongClick = { actOnChat(chat) },
                        )
                        .padding(
                            start = (40 + depth * 16).dp,
                            top = 10.dp,
                            bottom = 10.dp,
                            end = 16.dp,
                        ),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    if (chat.working) {
                        Dots(
                            MaterialTheme.colorScheme.outline,
                            contentDescription = "Working",
                        )
                    } else {
                        BackendIcon(chat.backend)
                    }
                    Spacer(Modifier.width(10.dp))
                    Text(
                        chat.title,
                        modifier = Modifier.weight(1f),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
        children[folder.id].orEmpty().forEach { child ->
            folderRows(
                child, depth + 1, children, chats, expanded, toggle, openChat, actOnChat,
            )
        }
    }
}
