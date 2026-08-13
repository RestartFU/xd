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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.net.Link

private const val MOBILE_UPDATE_URL =
    "https://github.com/RestartFU/xd/releases/download/nightly/xd-nightly-android.apk"

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TreeScreen(
    model: MainViewModel,
    link: Link,
    settings: MobileSettings,
    openChat: (String) -> Unit,
) {
    val uriHandler = LocalUriHandler.current
    val accent by settings.accent.collectAsStateWithLifecycle()
    val speechEnabled by settings.speechEnabled.collectAsStateWithLifecycle()
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()
    val moving by model.moving.collectAsStateWithLifecycle()
    val createdChat by model.createdChat.collectAsStateWithLifecycle()
    val shortcutEditor by model.shortcutEditor.collectAsStateWithLifecycle()
    var expandedIds by rememberSaveable { mutableStateOf(emptyList<String>()) }
    val roots = tree.folders.filter { it.parentId == null }
    val children = tree.folders.groupBy(Folder::parentId)
    val chats = tree.chats.groupBy(ChatSummary::folderId)
    val foldersById = tree.folders.associateBy(Folder::id)
    var choosingNew by rememberSaveable { mutableStateOf(false) }
    var choosingFolder by rememberSaveable { mutableStateOf(false) }
    var namingWorkspace by rememberSaveable { mutableStateOf(false) }
    var acting by rememberSaveable { mutableStateOf<Triple<String, String, String>?>(null) }
    var actingFolder by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var movingChat by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var movingFolder by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var renaming by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var deleting by rememberSaveable { mutableStateOf<Pair<String, String>?>(null) }
    var confirmingForget by rememberSaveable { mutableStateOf(false) }
    var showingSettings by rememberSaveable { mutableStateOf(false) }

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
                    IconButton(onClick = { showingSettings = true }) {
                        Icon(
                            painter = painterResource(R.drawable.ic_settings),
                            contentDescription = "App settings",
                        )
                    }
                    TextButton(onClick = { uriHandler.openUri(MOBILE_UPDATE_URL) }) {
                        Text("Update app")
                    }
                    TextButton(onClick = { confirmingForget = true }) {
                        Text("Forget")
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton(
                onClick = { choosingNew = true },
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
                            actOnFolder = { folder ->
                                if (!moving) actingFolder = folder.id to folder.name
                            },
                            actOnChat = { chat ->
                                if (!moving) acting = Triple(chat.id, chat.title, chat.folderId)
                            },
                        )
                    }
                }
            }
        }
    }
    acting?.let { (chatId, title, folderId) ->
        ChatActionsDialog(
            title = title,
            moveEnabled = !moving,
            onDismiss = { acting = null },
            onMove = {
                acting = null
                movingChat = chatId to folderId
            },
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
    actingFolder?.let { (folderId, name) ->
        FolderActionsDialog(
            name = name,
            moveEnabled = !moving,
            onDismiss = { actingFolder = null },
            onMove = {
                actingFolder = null
                movingFolder = folderId to name
            },
            onShortcuts = {
                actingFolder = null
                model.openShortcutEditor(folderId, "$name shortcuts")
            },
        )
    }
    movingChat?.let { (chatId, currentFolderId) ->
        MoveDestinationDialog(
            title = "Move chat to…",
            folders = tree.folders,
            foldersById = foldersById,
            currentParentId = currentFolderId,
            includeTopLevel = false,
            sourceName = null,
            onDismiss = { movingChat = null },
            onMove = { folderId ->
                movingChat = null
                if (folderId != null) model.moveChat(chatId, folderId)
            },
        )
    }
    movingFolder?.let { (folderId, name) ->
        foldersById[folderId]?.let { folder ->
            MoveDestinationDialog(
                title = "Move folder to…",
                folders = tree.folders,
                foldersById = foldersById,
                excludedIds = folderDescendants(folder.id, tree.folders),
                currentParentId = folder.parentId,
                includeTopLevel = true,
                sourceName = name,
                onDismiss = { movingFolder = null },
                onMove = { parentId ->
                    movingFolder = null
                    model.moveFolder(folderId, parentId)
                },
            )
        }
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
                    name + " and its messages will be removed on the host. " +
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
    if (choosingNew) {
        AlertDialog(
            onDismissRequest = { choosingNew = false },
            title = { Text("Create new") },
            text = {
                Column(Modifier.fillMaxWidth()) {
                    Text(
                        "Workspace",
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable {
                                choosingNew = false
                                namingWorkspace = true
                            }
                            .padding(vertical = 12.dp),
                    )
                    Text(
                        "Chat",
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable(enabled = tree.folders.isNotEmpty()) {
                                choosingNew = false
                                choosingFolder = true
                            }
                            .padding(vertical = 12.dp),
                        color = if (tree.folders.isEmpty()) {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        } else {
                            MaterialTheme.colorScheme.onSurface
                        },
                    )
                    if (tree.folders.isEmpty()) {
                        Text(
                            "Create a workspace before starting a chat.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { choosingNew = false }) { Text("Cancel") }
            },
        )
    }
    if (namingWorkspace) {
        CreateWorkspaceDialog(
            model = model,
            onDismiss = { namingWorkspace = false },
            onCreate = { name, repository ->
                namingWorkspace = false
                model.createWorkspace(name, repository)
            },
        )
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
    if (showingSettings) {
        MobileSettingsDialog(
            accent = accent,
            speechEnabled = speechEnabled,
            onAccentChanged = settings::setAccent,
            onSpeechChanged = settings::setSpeechEnabled,
            onShortcuts = {
                showingSettings = false
                model.openShortcutEditor(null, "Global shortcuts")
            },
            onDismiss = { showingSettings = false },
        )
    }
    shortcutEditor?.let { editor ->
        ShortcutEditorDialog(
            state = editor,
            onDismiss = model::closeShortcutEditor,
            onSave = model::saveShortcuts,
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
 * A list rather than dialog buttons: these are things to do, not a question with
 * a yes and a no, and a destructive action does not belong where a thumb expects
 * Confirm.
 */
@Composable
private fun ChatActionsDialog(
    title: String,
    moveEnabled: Boolean,
    onDismiss: () -> Unit,
    onMove: () -> Unit,
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
                    "Move",
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = moveEnabled, onClick = onMove)
                        .padding(vertical = 12.dp),
                    color = if (moveEnabled) {
                        MaterialTheme.colorScheme.onSurface
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
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
private fun FolderActionsDialog(
    name: String,
    moveEnabled: Boolean,
    onDismiss: () -> Unit,
    onMove: () -> Unit,
    onShortcuts: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                name,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        },
        text = {
            Column(Modifier.fillMaxWidth()) {
                Text(
                    "Prompt shortcuts",
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(onClick = onShortcuts)
                        .padding(vertical = 12.dp),
                )
                Text(
                    "Move",
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = moveEnabled, onClick = onMove)
                        .padding(vertical = 12.dp),
                    color = if (moveEnabled) {
                        MaterialTheme.colorScheme.onSurface
                    } else {
                        MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

@Composable
private fun MoveDestinationDialog(
    title: String,
    folders: List<Folder>,
    foldersById: Map<String, Folder>,
    excludedIds: Set<String> = emptySet(),
    currentParentId: String?,
    includeTopLevel: Boolean,
    sourceName: String?,
    onDismiss: () -> Unit,
    onMove: (String?) -> Unit,
) {
    val candidates = folders.filterNot { it.id in excludedIds }

    fun hasNameCollision(parentId: String?): Boolean =
        sourceName != null && folders.any {
            it.parentId == parentId && it.name == sourceName
        }

    fun enabled(parentId: String?): Boolean =
        parentId != currentParentId && !hasNameCollision(parentId)

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState()),
            ) {
                if (includeTopLevel) {
                    Text(
                        "Top level",
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable(enabled = enabled(null)) {
                                onMove(null)
                            }
                            .padding(vertical = 12.dp),
                        color = if (enabled(null)) {
                            MaterialTheme.colorScheme.onSurface
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
                if (candidates.isEmpty()) {
                    Text(
                        "No other workspace folders are available.",
                        modifier = Modifier.padding(vertical = 8.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                candidates.forEach { folder ->
                    val canMove = enabled(folder.id)
                    Text(
                        folderPath(folder, foldersById),
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable(enabled = canMove) { onMove(folder.id) }
                            .padding(vertical = 12.dp),
                        color = if (canMove) {
                            MaterialTheme.colorScheme.onSurface
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

@Composable
private fun CreateWorkspaceDialog(
    model: MainViewModel,
    onDismiss: () -> Unit,
    onCreate: (String, String?) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf("") }
    var repository by rememberSaveable { mutableStateOf<String?>(null) }
    var choosingRepository by rememberSaveable { mutableStateOf(false) }
    val cleaned = name.trim()
    val nameError = workspaceNameError(cleaned)

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New workspace") },
        text = {
            Column {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Name") },
                    singleLine = true,
                    isError = nameError != null,
                )
                nameError?.let {
                    Text(
                        it,
                        modifier = Modifier.padding(top = 4.dp),
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Text(
                    repository ?: "No Git repository selected",
                    modifier = Modifier.padding(top = 12.dp),
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Row {
                    TextButton(onClick = { choosingRepository = true }) {
                        Text("Choose repository")
                    }
                    if (repository != null) {
                        TextButton(onClick = { repository = null }) {
                            Text("Clear")
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onCreate(cleaned, repository) },
                enabled = cleaned.isNotEmpty() && nameError == null,
            ) { Text("Create") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )

    if (choosingRepository) {
        RepositoryPickerDialog(
            model = model,
            onDismiss = { choosingRepository = false },
            onChoose = {
                repository = it
                choosingRepository = false
            },
        )
    }
}

@Composable
private fun RepositoryPickerDialog(
    model: MainViewModel,
    onDismiss: () -> Unit,
    onChoose: (String) -> Unit,
) {
    var requestedPath by rememberSaveable { mutableStateOf<String?>(null) }
    var currentPath by rememberSaveable { mutableStateOf("") }
    var entries by remember { mutableStateOf(emptyList<String>()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(requestedPath) {
        loading = true
        error = null
        try {
            val result = model.client.listDirectories(requestedPath)
            currentPath = result.path
            entries = result.entries
        } catch (failure: Throwable) {
            error = failure.message ?: "Could not list that directory"
            entries = emptyList()
        } finally {
            loading = false
        }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Choose Git repository") },
        text = {
            Column {
                Row(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        currentPath.ifBlank { "Loading…" },
                        modifier = Modifier.weight(1f),
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
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
                }
                if (loading) {
                    CircularProgressIndicator(
                        modifier = Modifier.padding(vertical = 20.dp),
                    )
                } else if (entries.isEmpty()) {
                    Text(
                        "No subfolders are available.",
                        modifier = Modifier.padding(vertical = 20.dp),
                    )
                } else {
                    LazyColumn(Modifier.heightIn(max = 300.dp)) {
                        items(entries, key = { it }) { entry ->
                            Text(
                                entry,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        requestedPath = joinDirectory(currentPath, entry)
                                    }
                                    .padding(vertical = 12.dp),
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onChoose(currentPath) },
                enabled = !loading && currentPath.isNotBlank(),
            ) { Text("Use this folder") }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

private fun joinDirectory(parent: String, child: String): String = when {
    parent.endsWith('/') || parent.endsWith('\\') -> parent + child
    else -> "$parent/$child"
}

private fun parentDirectory(path: String): String? {
    if (path.isBlank()) return null
    val trimmed = path.trimEnd('/', '\\')
    if (trimmed.isEmpty()) return path.take(1)
    val separator = maxOf(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
    if (separator < 0) return null
    if (separator == 0) return trimmed.substring(0, 1)
    if (separator == 2 && trimmed.getOrNull(1) == ':') {
        return trimmed.substring(0, 3)
    }
    return trimmed.substring(0, separator)
}

private fun workspaceNameError(name: String): String? = when {
    name.isEmpty() -> null
    name.startsWith('.') -> "A workspace name cannot begin with a period."
    '/' in name || '\\' in name -> "A workspace name cannot contain a path separator."
    else -> null
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
                // The host refuses a blank title rather than clearing it, so
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

/** Guards against a cycle in host-supplied parent ids. */
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

/** Includes [sourceId] so a folder cannot be moved into itself or its subtree. */
private fun folderDescendants(
    sourceId: String,
    folders: List<Folder>,
): Set<String> {
    val children = folders.groupBy(Folder::parentId)
    val excluded = mutableSetOf<String>()
    val pending = ArrayDeque<String>()
    pending.add(sourceId)
    while (pending.isNotEmpty()) {
        val id = pending.removeFirst()
        if (!excluded.add(id)) continue
        children[id].orEmpty().forEach { child -> pending.add(child.id) }
    }
    return excluded
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
    actOnFolder: (Folder) -> Unit,
    actOnChat: (ChatSummary) -> Unit,
) {
    item(key = "folder-${folder.id}") {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .combinedClickable(
                    onClick = { toggle(folder.id) },
                    onLongClick = { actOnFolder(folder) },
                )
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
                child, depth + 1, children, chats, expanded, toggle, openChat,
                actOnFolder, actOnChat,
            )
        }
    }
}
