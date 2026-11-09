@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.width
import androidx.activity.compose.BackHandler
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.ViewModel
import com.restartfu.xd.model.ChatSummary
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.ui.unit.Dp
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.animation.core.animateFloat
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.model.MinimalProject
import com.restartfu.xd.model.MinimalSession
import com.restartfu.xd.model.backgroundTasksLabel
import com.restartfu.xd.model.minimalProjects
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import com.restartfu.xd.model.FolderDestination
import com.restartfu.xd.model.MinimalFolder
import com.restartfu.xd.model.ProjectTreeRow
import com.restartfu.xd.model.ancestors
import com.restartfu.xd.model.moveDestinations
import com.restartfu.xd.model.organizationLogin
import com.restartfu.xd.model.projectTree
import com.restartfu.xd.model.minimalSessions
import com.restartfu.xd.net.Link
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarResult
import com.restartfu.xd.model.SettledSession
import com.restartfu.xd.model.activeSessions
import com.restartfu.xd.model.settledSessions
import com.restartfu.xd.protocol.Ops
import androidx.compose.material3.RadioButton
import com.restartfu.xd.protocol.BranchSourceReply
import com.restartfu.xd.protocol.PullRequestSourceReply
import com.restartfu.xd.protocol.WorktreeReply
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch

private enum class MobileDestination {
    PROJECTS,
    SESSIONS,
    TERMINAL,
    USAGE,
}

private const val GLOBAL_TERMINAL_CHAT_ID = "global:mobile"

private val ChatSelectionsSaver = listSaver<Map<String, String>, String>(
    save = { selected -> selected.flatMap { listOf(it.key, it.value) } },
    restore = { saved -> saved.chunked(2).filter { it.size == 2 }.associate { it[0] to it[1] } },
)

/** Retains chat drafts and shared workspace panes when a tab leaves composition. */
internal class MobileWorkspaceModels : ViewModel() {
    private val chats = mutableMapOf<String, SessionViewModelOwner>()
    private val workspaces = mutableMapOf<String, SessionViewModelOwner>()
    private var generation: Long? = null

    fun bindGeneration(value: Long) {
        if (generation == value) return
        clearStores()
        generation = value
    }

    fun chat(id: String): ViewModelStoreOwner = chats.getOrPut(id) { SessionViewModelOwner() }
    fun workspace(id: String): ViewModelStoreOwner = workspaces.getOrPut(id) { SessionViewModelOwner() }
    private var usageOwner: SessionViewModelOwner? = null
    fun usage(): ViewModelStoreOwner = usageOwner ?: SessionViewModelOwner().also { usageOwner = it }

    fun retain(chatIds: Set<String>, sessionIds: Set<String>) {
        // Panes can reference a chat connection, so release them first.
        workspaces.keys.filterNot { it in sessionIds }.forEach { workspaces.remove(it)?.viewModelStore?.clear() }
        chats.keys.filterNot { it in chatIds }.forEach { chats.remove(it)?.viewModelStore?.clear() }
    }

    private fun clearStores() {
        usageOwner?.viewModelStore?.clear()
        usageOwner = null
        workspaces.values.forEach { it.viewModelStore.clear() }
        chats.values.forEach { it.viewModelStore.clear() }
        workspaces.clear()
        chats.clear()
    }

    override fun onCleared() = clearStores()
}

@Composable
internal fun MinimalMobileApp(
    model: MainViewModel,
    link: Link,
    settings: MobileSettings,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val created by model.createdDirectSession.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    val stores: MobileWorkspaceModels = viewModel()
    val sessionGeneration by model.client.sessionGeneration.collectAsStateWithLifecycle()
    remember(stores, sessionGeneration) { stores.bindGeneration(sessionGeneration) }
    val savedChatState = rememberSaveableStateHolder()
    var activeSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var chatSelections by rememberSaveable(stateSaver = ChatSelectionsSaver) { mutableStateOf(emptyMap()) }
    var hiddenChatIds by rememberSaveable { mutableStateOf(emptyList<String>()) }
    var mobileDestination by rememberSaveable { mutableStateOf(MobileDestination.PROJECTS) }
    var createChatIn by remember { mutableStateOf<MinimalSession?>(null) }
    // Saved by id, so a chat rename survives rotating the phone.
    var renameChatId by rememberSaveable { mutableStateOf<String?>(null) }
    val renameChat = tree.chats.firstOrNull { it.id == renameChatId }
    var deleteChat by remember { mutableStateOf<ChatSummary?>(null) }
    val deleting by model.deletingChat.collectAsStateWithLifecycle()
    val renamingChats by model.renamingChat.collectAsStateWithLifecycle()
    // Saved by id, so a rename in progress survives rotating the phone.
    var renamingSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingSession by remember { mutableStateOf<MinimalSession?>(null) }
    var renamingProjectId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingProject by remember { mutableStateOf<MinimalProject?>(null) }
    val settledNotice by model.settledNotice.collectAsStateWithLifecycle()
    val sessionActions = SessionActions(
        settlement = tree.settlement,
        settle = { model.settleSession(it.id, it.title) },
        unsettle = { model.unsettleSession(it.id) },
        setAutoSettle = { session, enabled -> model.setSessionAutoSettle(session.id, enabled) },
        rename = { renamingSessionId = it.id },
        delete = { deletingSession = it },
    )
    val projects = tree.minimalProjects()
    val sessions = projects.flatMap { project -> tree.minimalSessions(project.id) }
    val activeSession = sessions.firstOrNull { it.id == activeSessionId }
    val activeProject = projects.firstOrNull { project ->
        tree.minimalSessions(project.id).any { it.id == activeSessionId }
    }
    val sessionChats = activeSession?.chatIds.orEmpty().mapNotNull { id -> tree.chats.firstOrNull { it.id == id } }
    val openChats = sessionChats.filterNot { it.id in hiddenChatIds }
    val activeChat = openChats.firstOrNull { it.id == chatSelections[activeSessionId] } ?: openChats.firstOrNull()

    LaunchedEffect(tree.chats, tree.loading, tree.error) {
        if (!tree.loading && tree.error == null) {
            stores.retain(tree.chats.map { it.id }.toSet(), sessions.map { it.id }.toSet())
        }
    }
    LaunchedEffect(created, tree.chats) {
        created?.let { result ->
            val chat = tree.chats.firstOrNull { it.id == result.chatId } ?: return@let
            activeSessionId = chat.sessionId
            chatSelections = chatSelections + (chat.sessionId to chat.id)
            hiddenChatIds = hiddenChatIds - chat.id
            mobileDestination = MobileDestination.SESSIONS
            model.consumeCreatedDirectSession(result)
        }
    }
    val visibleChatId = activeChat?.id.takeIf { mobileDestination == MobileDestination.SESSIONS }
    DisposableEffect(visibleChatId) {
        model.client.setVisibleChat(visibleChatId)
        onDispose { model.client.setVisibleChat(null) }
    }
    val showProjects = { mobileDestination = MobileDestination.PROJECTS }
    val showSessions = {
        if (activeSession == null) activeSessionId = (sessions.firstOrNull { !it.settled } ?: sessions.firstOrNull())?.id
        mobileDestination = if (sessions.isEmpty()) MobileDestination.PROJECTS else MobileDestination.SESSIONS
    }
    val showTerminal = { mobileDestination = MobileDestination.TERMINAL }
    val showUsage = { mobileDestination = MobileDestination.USAGE }
    val addChat: (MinimalSession) -> Unit = { session ->
        if (session.terminalChatId.startsWith("session:")) {
            createChatIn = session
        } else {
            scope.launch { snackbar.showSnackbar("Update the connected host to add chats inside a session.") }
        }
    }
    BackHandler(enabled = mobileDestination != MobileDestination.PROJECTS, onBack = showProjects)
    val expanded = LocalConfiguration.current.screenWidthDp >= 600
    val imeVisible = WindowInsets.isImeVisible
    val chatOpen = mobileDestination == MobileDestination.SESSIONS && activeChat != null
    val showBottomNavigation = !expanded && !imeVisible && !chatOpen

    LaunchedEffect(operationError) {
        operationError?.let { snackbar.showSnackbar(it) }
    }
    // A session settled by hand can come straight back, as on the desktop.
    LaunchedEffect(settledNotice) {
        val notice = settledNotice ?: return@LaunchedEffect
        if (notice.sessionId == activeSessionId && mobileDestination == MobileDestination.SESSIONS) {
            mobileDestination = MobileDestination.PROJECTS
        }
        val result = snackbar.showSnackbar(
            message = "Settled ${notice.title}",
            actionLabel = "Undo",
            duration = SnackbarDuration.Short,
        )
        if (result == SnackbarResult.ActionPerformed) model.undoSettle(notice)
        model.consumeSettledNotice(notice)
    }
    Box(Modifier.fillMaxSize()) {
    Row(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        if (expanded) {
            XdNavigationRail(mobileDestination, showProjects, showSessions, showTerminal, showUsage)
        }
        Column(Modifier.weight(1f)) {
            Box(Modifier.weight(1f)) {
                when {
                    mobileDestination == MobileDestination.TERMINAL -> MinimalGlobalTerminal(
                        model, settings, link, showProjects, showSessions,
                    )
                    mobileDestination == MobileDestination.USAGE -> Column(
                        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
                    ) {
                        ProductHeader(link = link, title = "Usage", onSettings = { settingsOpen = true })
                        // Kept with the other per-host stores, so leaving Usage keeps what it read.
                        val usage: UsageViewModel = viewModel(
                            key = "usage",
                            viewModelStoreOwner = stores.usage(),
                            factory = UsageViewModel.Factory(model.client, settings),
                        )
                        UsageContent(usage)
                    }
                    mobileDestination == MobileDestination.SESSIONS && activeSession != null -> {
                        val selectTab = { id: String ->
                            hiddenChatIds = hiddenChatIds - id
                            chatSelections = chatSelections + (activeSession.id to id)
                        }
                        val closeTab = { id: String ->
                            val index = openChats.indexOfFirst { it.id == id }
                            val remaining = openChats.filterNot { it.id == id }
                            if (id == activeChat?.id && remaining.isNotEmpty()) {
                                chatSelections = chatSelections + (activeSession.id to remaining[index.coerceAtMost(remaining.lastIndex)].id)
                            }
                            hiddenChatIds = hiddenChatIds + id
                        }
                        val tabStrip: @Composable () -> Unit = {
                            SessionTabStrip(
                                compact = !expanded,
                                chats = sessionChats,
                                hiddenChatIds = hiddenChatIds,
                                selectedChatId = activeChat?.id,
                                selectTab = selectTab,
                                closeTab = closeTab,
                                addTab = { addChat(activeSession) },
                                renameChat = { renameChatId = it.id },
                                deleteChat = { deleteChat = it },
                                session = activeSession,
                                actions = sessionActions,
                            )
                        }
                        if (activeChat != null) {
                            savedChatState.SaveableStateProvider(activeChat.id) {
                                val chat: ChatViewModel = viewModel(
                                    key = "chat-${activeChat.id}",
                                    viewModelStoreOwner = stores.chat(activeChat.id),
                                    factory = ChatViewModel.Factory(model.client, activeChat.id),
                                )
                                ChatScreen(
                                    model = chat, settings = settings, goBack = showProjects,
                                    sessionTitle = activeSession.title,
                                    branch = activeSession.branch,
                                    sessionWorking = activeSession.working,
                                    backgroundTasks = activeChat.backgroundTasks,
                                    projectName = activeProject?.name.orEmpty(),
                                    workspaceId = activeSession.id,
                                    terminalChatId = activeSession.terminalChatId,
                                    workspaceOwner = stores.workspace(activeSession.id),
                                    sessionTabs = tabStrip,
                                    // A phone keeps one header row and moves the
                                    // tabs into it; a tablet has room for the tabs.
                                    sessionChats = if (expanded) null else SessionChats(
                                        chats = sessionChats,
                                        selectedId = activeChat.id,
                                        select = selectTab,
                                        add = { addChat(activeSession) },
                                        close = closeTab,
                                        rename = { renameChatId = it.id },
                                        delete = { deleteChat = it },
                                        settleSession = { sessionActions.settle(activeSession) }
                                            .takeIf { sessionActions.settlement && !activeSession.settled },
                                        renameSession = { sessionActions.rename(activeSession) },
                                    ),
                                )
                            }
                        } else {
                            Column(Modifier.fillMaxSize()) {
                                ProductHeader(link, activeSession.title, onSettings = { settingsOpen = true })
                                tabStrip()
                                Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
                                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                        Text("Your chats are saved", style = MaterialTheme.typography.titleLarge)
                                        Text("Reopen a chat from the tab menu, or start a new one.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        Button(onClick = { addChat(activeSession) }) { Text("New chat") }
                                    }
                                }
                            }
                        }
                    }
                    else -> MinimalProjectsHome(
                        model, settings, link, projects, tree.loading, tree.error, showSessions, showTerminal,
                        actions = sessionActions,
                        onRenameProject = { renamingProjectId = it.id },
                        onDeleteProject = { deletingProject = it },
                        open = { _, session ->
                            activeSessionId = session.id
                            // Opening a session always reveals at least its most recent chat.
                            if (session.chatIds.all { it in hiddenChatIds }) {
                                hiddenChatIds = hiddenChatIds - session.chatIds.first()
                            }
                            mobileDestination = MobileDestination.SESSIONS
                        },
                    )
                }
            }
            if (showBottomNavigation) {
                XdNavigationBar(mobileDestination, showProjects, showSessions, showTerminal, showUsage)
            }
        }
    }
    SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(horizontal = 16.dp, vertical = if (showBottomNavigation) 96.dp else 24.dp))
    }
    if (settingsOpen) {
        MinimalSettingsDialog(model, settings, onDismiss = { settingsOpen = false }, onDisconnect = {
            settingsOpen = false
            model.forget()
        })
    }
    val shortcutEditor by model.shortcutEditor.collectAsStateWithLifecycle()
    shortcutEditor?.let { editor ->
        ShortcutEditorDialog(
            state = editor,
            onDismiss = model::closeShortcutEditor,
            onSave = model::saveShortcuts,
            onRetry = { model.openShortcutEditor(editor.folderId, editor.title) },
            onText = { id, text -> model.setShortcutText(editor.opening, id, text) },
            onAdd = { model.addShortcut(editor.opening) },
            onRemove = { id -> model.removeShortcut(editor.opening, id) },
        )
    }
    sessions.firstOrNull { it.id == renamingSessionId }?.let { session ->
        NameDialog(
            title = "Rename session",
            target = "session:${session.id}",
            initial = session.title,
            // An older host's session is renamed as its chat, one at a time.
            busy = !session.grouped && renamingChats,
            onDismiss = { renamingSessionId = null },
            onConfirm = { name ->
                renamingSessionId = null
                if (name != session.title) {
                    // An older host's "session" is a single chat under the chat's own id.
                    if (session.grouped) model.renameSession(session.id, name) else model.renameChat(session.id, name)
                }
            },
        )
    }
    deletingSession?.let { session ->
        ConfirmDeleteDialog(
            title = "Delete session?",
            busy = !session.grouped && deleting,
            text = "This permanently removes ${session.title} and all its chats from xd. Project files are kept.",
            confirm = "Delete session",
            onDismiss = { deletingSession = null },
            onConfirm = {
                deletingSession = null
                if (session.id == activeSessionId) mobileDestination = MobileDestination.PROJECTS
                if (session.grouped) model.deleteSession(session.id) else model.deleteChat(session.id)
            },
        )
    }
    projects.firstOrNull { it.id == renamingProjectId }?.let { project ->
        NameDialog(
            title = "Rename project",
            target = "project:${project.id}",
            initial = project.name,
            validate = ::projectNameError,
            onDismiss = { renamingProjectId = null },
            onConfirm = { name ->
                renamingProjectId = null
                if (name != project.name) model.renameProject(project.id, name)
            },
        )
    }
    deletingProject?.let { project ->
        ConfirmDeleteDialog(
            title = "Delete project?",
            text = "This removes ${project.name} and all its sessions from xd. Its files will be moved to the workspace trash.",
            confirm = "Delete project",
            onDismiss = { deletingProject = null },
            onConfirm = {
                deletingProject = null
                if (activeProject?.id == project.id) mobileDestination = MobileDestination.PROJECTS
                model.deleteProject(project.id)
            },
        )
    }
    renameChat?.let { chat ->
        // The same dialog as sessions and projects: its draft is saved with the
        // chat it renames, and it waits while an earlier rename is still out.
        NameDialog(
            title = "Rename chat",
            target = "chat:${chat.id}",
            initial = chat.title,
            busy = renamingChats,
            onDismiss = { renameChatId = null },
            onConfirm = { title ->
                renameChatId = null
                if (title != chat.title) model.renameChat(chat.id, title)
            },
        )
    }
    deleteChat?.let { chat ->
        AlertDialog(
            onDismissRequest = { if (!deleting) deleteChat = null },
            title = { Text("Delete this chat?") },
            text = { Text("This permanently deletes “${chat.title}” and its history. Other chats in this session stay saved.") },
            confirmButton = {
                TextButton(onClick = { model.deleteChat(chat.id); deleteChat = null }, enabled = !deleting) {
                    Text("Delete", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = { TextButton(onClick = { deleteChat = null }, enabled = !deleting) { Text("Cancel") } },
        )
    }
    createChatIn?.let { session ->
        LaunchedEffect(session.id) { model.loadCreationCatalog() }
        NewSessionDialog(
            projectName = session.title,
            draftKey = "${activeProject?.id}/${session.id}",
            backends = model.hostCatalog.collectAsStateWithLifecycle().value,
            catalogError = model.hostCatalogError.collectAsStateWithLifecycle().value,
            retryCatalog = model::loadCreationCatalog,
            chatTab = true,
            onDismiss = { createChatIn = null },
            onCreate = { title, agent, _, effort, access, plan ->
                createChatIn = null
                activeProject?.let { project -> model.createDirectSession(project.id, title, agent, session.id, effort = effort, access = access, plan = plan) }
            },
        )
    }
}

@Composable
private fun MinimalProjectsHome(
    model: MainViewModel,
    settings: MobileSettings,
    link: Link,
    projects: List<MinimalProject>,
    loading: Boolean,
    treeError: String?,
    onSessions: () -> Unit,
    onTerminal: () -> Unit,
    actions: SessionActions,
    onRenameProject: (MinimalProject) -> Unit,
    onDeleteProject: (MinimalProject) -> Unit,
    open: (MinimalProject, MinimalSession) -> Unit,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    var shelfOpen by rememberSaveable { mutableStateOf(false) }
    // Saved by project id, so an open editor survives recreating the activity.
    var contextForId by rememberSaveable { mutableStateOf<String?>(null) }
    var secretsForId by rememberSaveable { mutableStateOf<String?>(null) }
    // Folders of projects have context and secrets too, which reach every project inside.
    val contextFor = tree.folders.firstOrNull { it.id == contextForId }
    val secretsFor = tree.folders.firstOrNull { it.id == secretsForId }
    val settled = if (actions.settlement) tree.settledSessions() else emptyList()
    val operationError by model.error.collectAsStateWithLifecycle()
    var selectedId by rememberSaveable { mutableStateOf<String?>(null) }
    // What the New menu or a folder's menu is making ("project" or "folder"), and in which folder.
    var creating by rememberSaveable { mutableStateOf<String?>(null) }
    var createParentId by rememberSaveable { mutableStateOf<String?>(null) }
    var renamingFolderId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingFolderId by rememberSaveable { mutableStateOf<String?>(null) }
    var movingId by rememberSaveable { mutableStateOf<String?>(null) }
    val collapsed by settings.collapsedFolders.collectAsStateWithLifecycle()
    // An earlier project or folder is still being made; another waits rather than being dropped.
    val creatingWorkspace by model.creatingWorkspace.collectAsStateWithLifecycle()
    val ownerIcons by model.client.ownerIcons.collectAsStateWithLifecycle()
    val rows = tree.projectTree(collapsed)
    val folders = tree.projectTree().mapNotNull { (it as? ProjectTreeRow.FolderRow)?.folder }
    val shownProjects = rows.mapNotNull { (it as? ProjectTreeRow.ProjectRow)?.project }
    // Opens the folders [id] sits in, and [id] itself, so what lands there is in view.
    val reveal = { id: String? ->
        if (id != null) (tree.ancestors(id) + id).forEach { settings.setFolderOpen(it, true) }
    }
    val newProjectIn = { parentId: String? -> createParentId = parentId; creating = "project" }
    val newFolderIn = { parentId: String? -> createParentId = parentId; creating = "folder" }
    val canMove = { id: String, parentId: String? -> parentId != null || tree.moveDestinations(id).isNotEmpty() }
    // The project the dialog was opened for; it closes if that project goes away.
    var createSessionIn by rememberSaveable { mutableStateOf<String?>(null) }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }

    // A project closed away in a folder stays chosen, but shows nothing until its folder opens.
    val chosen = projects.firstOrNull { it.id == selectedId }
    val selected = if (chosen != null) chosen.takeIf { it in shownProjects } else shownProjects.firstOrNull()
    val sessions = selected?.let { tree.activeSessions(it.id) }.orEmpty()
    val waiting by model.client.waitingChats.collectAsStateWithLifecycle()
    val unread by model.client.unreadChats.collectAsStateWithLifecycle()
    val signalOf = { session: MinimalSession ->
        when {
            session.chatIds.any { it in waiting } -> SessionSignal.WAITING
            session.working -> SessionSignal.WORKING
            session.backgroundTasks > 0 -> SessionSignal.BACKGROUND
            session.chatIds.any { it in unread } -> SessionSignal.DONE
            else -> SessionSignal.IDLE
        }
    }

    LaunchedEffect(selected?.id, chosen == null) {
        if (chosen == null && selectedId != selected?.id) selectedId = selected?.id
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .navigationBarsPadding(),
    ) {
        ProductHeader(link = link, title = "Projects", onSettings = { settingsOpen = true })
        ContextBar(title = countLabel(projects.size, folders.size)) {
            NewMenu(
                canCreateFolders = tree.canCreateFolders,
                onProject = { newProjectIn(null) },
                onFolder = { newFolderIn(null) },
            )
        }

        when {
            loading && projects.isEmpty() -> Box(
                Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }

            else -> Box(Modifier.fillMaxSize()) { LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp, 14.dp, 12.dp, 96.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                (treeError ?: operationError)?.let { error ->
                    item("error") {
                        Text(
                            error,
                            modifier = Modifier.padding(horizontal = 6.dp, vertical = 8.dp),
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
                if (rows.isEmpty()) {
                    item("empty-projects") {
                        EmptyCard("Create a workspace, then start a Codex, Claude, or JCode session.")
                    }
                } else {
                    items(rows, key = ProjectTreeRow::id) { row ->
                        val indent = Modifier.padding(start = (row.depth * FOLDER_INDENT).dp)
                        when (row) {
                            is ProjectTreeRow.FolderRow -> FolderRow(
                                folder = row.folder,
                                open = row.open,
                                icon = ownerIcons[row.folder.owner],
                                modifier = indent,
                                toggle = { settings.setFolderOpen(row.folder.id, !row.open) },
                                menu = {
                                    FolderMenu(
                                        folder = row.folder,
                                        canCreateFolders = tree.canCreateFolders,
                                        onNewProject = { newProjectIn(row.folder.id) },
                                        onNewFolder = { newFolderIn(row.folder.id) },
                                        onMove = { movingId = row.folder.id }
                                            .takeIf { canMove(row.folder.id, row.folder.parentId) },
                                        onRename = { renamingFolderId = row.folder.id },
                                        onShortcuts = {
                                            model.openShortcutEditor(row.folder.id, "${row.folder.name} shortcuts")
                                        },
                                        onContext = { contextForId = row.folder.id },
                                        onSecrets = { secretsForId = row.folder.id },
                                        onDelete = { deletingFolderId = row.folder.id },
                                    )
                                },
                            )
                            is ProjectTreeRow.ProjectRow -> Box(indent) {
                                val project = row.project
                                if (project.id == selected?.id) {
                                    OpenDrawer(
                                        project = project,
                                        icon = ownerIcons[project.owner],
                                        sessions = sessions,
                                        signalOf = signalOf,
                                        open = { session -> open(project, session) },
                                        actions = actions,
                                        menu = {
                                            ProjectMenu(
                                                project = project,
                                                onShortcuts = {
                                                    model.openShortcutEditor(project.id, "${project.name} shortcuts")
                                                },
                                                onRename = { onRenameProject(project) },
                                                onMove = { movingId = project.id }
                                                    .takeIf { canMove(project.id, project.parentId) },
                                                onDelete = { onDeleteProject(project) },
                                                onContext = { contextForId = project.id },
                                                onSecrets = { secretsForId = project.id },
                                            )
                                        },
                                    )
                                } else {
                                    DrawerRow(
                                        project = project,
                                        selected = false,
                                        icon = ownerIcons[project.owner],
                                        signals = tree.activeSessions(project.id).map(signalOf),
                                        onClick = { selectedId = project.id },
                                    )
                                }
                            }
                        }
                    }
                }
                if (settled.isNotEmpty()) {
                    item("settled-shelf") {
                        SettledShelf(
                            settled = settled,
                            expanded = shelfOpen,
                            toggle = { shelfOpen = !shelfOpen },
                            open = { open(it.project, it.session) },
                            actions = actions,
                        )
                    }
                }
            }
            if (selected != null) {
                ExtendedFloatingActionButton(
                    onClick = { createSessionIn = selected.id },
                    modifier = Modifier
                        .align(Alignment.BottomEnd)
                        .padding(16.dp),
                    icon = { Icon(painterResource(R.drawable.ic_add), contentDescription = null) },
                    text = { Text("New session") },
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                )
            }
            }
        }
    }

    val createParent = folders.firstOrNull { it.id == createParentId }
    LaunchedEffect(creating, createParentId, createParent == null, loading) {
        // The folder it was opened in went away; nothing is made anywhere else instead.
        if (creating != null && createParentId != null && createParent == null && !loading) creating = null
    }
    when (creating) {
        "project" -> NewProjectDialog(
            model = model,
            parentName = createParent?.name,
            busy = creatingWorkspace,
            onDismiss = { creating = null },
            onCreate = { name, repository ->
                creating = null
                reveal(createParentId)
                model.createWorkspace(name, repository, createParentId)
            },
        )
        "folder" -> NewFolderDialog(
            parentName = createParent?.name,
            busy = creatingWorkspace,
            onDismiss = { creating = null },
            onCreate = { name, owner ->
                creating = null
                reveal(createParentId)
                model.createProjectFolder(name, createParentId, owner)
            },
        )
    }
    folders.firstOrNull { it.id == renamingFolderId }?.let { folder ->
        NameDialog(
            title = "Rename folder",
            target = "folder:${folder.id}",
            initial = folder.name,
            validate = ::folderNameError,
            onDismiss = { renamingFolderId = null },
            onConfirm = { name ->
                renamingFolderId = null
                if (name != folder.name) model.renameProjectFolder(folder.id, name)
            },
        )
    }
    folders.firstOrNull { it.id == deletingFolderId }?.let { folder ->
        ConfirmDeleteDialog(
            title = "Delete folder?",
            text = "This removes ${folder.name}, every project and folder in it, and all their sessions from xd. " +
                "Its files will be moved to the workspace trash.",
            confirm = "Delete folder",
            onDismiss = { deletingFolderId = null },
            onConfirm = {
                deletingFolderId = null
                model.deleteProjectFolder(folder.id)
            },
        )
    }
    tree.folders.firstOrNull { it.id == movingId }?.let { item ->
        key(item.id) {
            MoveDialog(
                name = item.name,
                current = item.parentId?.takeIf { parent -> tree.folders.any { it.id == parent } },
                destinations = tree.moveDestinations(item.id),
                icons = ownerIcons,
                onDismiss = { movingId = null },
                onMove = { parentId ->
                    movingId = null
                    reveal(parentId)
                    model.moveIntoFolder(item.id, parentId)
                },
            )
        }
    }
    contextFor?.let { project ->
        key(project.id) {
            ProjectContextDialog(model, project.id, project.name, onDismiss = { contextForId = null })
        }
    }
    secretsFor?.let { project ->
        key(project.id) {
            AgentSecretsDialog(model, project.id, "${project.name} secrets", onDismiss = { secretsForId = null })
        }
    }
    val creatingIn = projects.firstOrNull { it.id == createSessionIn }
    LaunchedEffect(createSessionIn, creatingIn == null, loading) {
        if (createSessionIn != null && creatingIn == null && !loading) createSessionIn = null
    }
    if (creatingIn != null) key(creatingIn.id) {
        LaunchedEffect(creatingIn.id) { model.loadCreationCatalog() }
        NewSessionDialog(
            projectName = creatingIn.name,
            draftKey = "${creatingIn.id}/new",
            backends = model.hostCatalog.collectAsStateWithLifecycle().value,
            catalogError = model.hostCatalogError.collectAsStateWithLifecycle().value,
            retryCatalog = model::loadCreationCatalog,
            places = { NewSessionPlaces.load(model.client, creatingIn.id) },
            onDismiss = { createSessionIn = null },
            onCreate = { title, agent, worktree, effort, access, plan ->
                createSessionIn = null
                model.createDirectSession(creatingIn.id, title, agent, worktree = worktree, effort = effort, access = access, plan = plan)
            },
        )
    }
    if (settingsOpen) {
        MinimalSettingsDialog(
            model = model,
            settings = settings,
            onDismiss = { settingsOpen = false },
            onDisconnect = {
                settingsOpen = false
                model.forget()
            },
        )
    }
}

@Composable
private fun MinimalGlobalTerminal(
    model: MainViewModel,
    settings: MobileSettings,
    link: Link,
    onProjects: () -> Unit,
    onSessions: () -> Unit,
) {
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    val terminalOwner = remember { SessionViewModelOwner() }
    DisposableEffect(terminalOwner) {
        onDispose { terminalOwner.viewModelStore.clear() }
    }
    val terminal: TerminalViewModel = viewModel(
        key = "global-shell",
        viewModelStoreOwner = terminalOwner,
        factory = TerminalViewModel.ShellFactory(
            client = model.client,
            chatId = GLOBAL_TERMINAL_CHAT_ID,
        ),
    )

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .navigationBarsPadding(),
    ) {
        ProductHeader(link = link, title = "Terminal", onSettings = { settingsOpen = true })
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(54.dp)
                .background(MaterialTheme.colorScheme.surfaceContainerLow)
                .padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Terminal", modifier = Modifier.weight(1f), fontWeight = FontWeight.SemiBold)
            TextButton(onClick = terminal::kill) {
                Text("Stop", color = MaterialTheme.colorScheme.error)
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Box(
            modifier = Modifier
                .fillMaxSize()
                .padding(10.dp)
                .clip(RoundedCornerShape(14.dp))
                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(14.dp)),
        ) {
            TerminalPaneContent(terminal, showSessionBar = false)
        }
    }

    if (settingsOpen) {
        MinimalSettingsDialog(
            model = model,
            settings = settings,
            onDismiss = { settingsOpen = false },
            onDisconnect = {
                settingsOpen = false
                model.forget()
            },
        )
    }
}

@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun SessionTabStrip(
    compact: Boolean,
    chats: List<ChatSummary>,
    hiddenChatIds: List<String>,
    selectedChatId: String?,
    selectTab: (String) -> Unit,
    closeTab: (String) -> Unit,
    addTab: () -> Unit,
    renameChat: (ChatSummary) -> Unit,
    deleteChat: (ChatSummary) -> Unit,
    session: MinimalSession,
    actions: SessionActions,
) {
    var openMenu by remember { mutableStateOf(false) }
    Row(
        modifier = Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainerLow).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            modifier = Modifier.weight(1f).horizontalScroll(rememberScrollState()).padding(vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            chats.filterNot { it.id in hiddenChatIds }.forEach { chat ->
                val selected = chat.id == selectedChatId
                val reveal = remember(chat.id) { BringIntoViewRequester() }
                LaunchedEffect(chat.id, selected) {
                    if (selected) {
                        androidx.compose.runtime.withFrameNanos { }
                        reveal.bringIntoView()
                    }
                }
                Surface(
                    modifier = Modifier.bringIntoViewRequester(reveal),
                    shape = RoundedCornerShape(12.dp),
                    color = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                    border = if (selected) androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant) else null,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Row(
                            modifier = Modifier.heightIn(min = 48.dp).clickable(role = Role.Tab) { selectTab(chat.id) }.padding(start = 12.dp, end = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            BackendIcon(chat.backend, size = 16.dp)
                            Text(chat.title.ifBlank { "New chat" }, modifier = Modifier.widthIn(max = 140.dp),
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.labelLarge,
                                color = if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        if (!compact) {
                            IconButton(onClick = { closeTab(chat.id) }, modifier = Modifier.size(48.dp)) {
                                Icon(painterResource(R.drawable.ic_close), "Close ${chat.title}", modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
        }
        IconButton(onClick = addTab, modifier = Modifier.size(48.dp)) {
            Icon(painterResource(R.drawable.ic_add), "New chat")
        }
        Box {
            IconButton(onClick = { openMenu = true }, modifier = Modifier.size(48.dp)) {
                Icon(painterResource(R.drawable.ic_sessions), "All chats")
            }
            DropdownMenu(expanded = openMenu, onDismissRequest = { openMenu = false }) {
                chats.forEach { chat ->
                    DropdownMenuItem(
                        text = { Text(chat.title.ifBlank { "New chat" }, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        leadingIcon = { BackendIcon(chat.backend, size = 18.dp) },
                        onClick = { openMenu = false; selectTab(chat.id) },
                    )
                }
                chats.firstOrNull { it.id == selectedChatId }?.let { active ->
                    HorizontalDivider()
                    DropdownMenuItem(
                        text = { Text("Close chat") },
                        onClick = { openMenu = false; closeTab(active.id) },
                    )
                    DropdownMenuItem(
                        text = { Text("Rename chat") },
                        onClick = { openMenu = false; renameChat(active) },
                    )
                    DropdownMenuItem(
                        text = { Text("Delete chat…", color = MaterialTheme.colorScheme.error) },
                        onClick = { openMenu = false; deleteChat(active) },
                    )
                }
                HorizontalDivider()
                if (actions.settlement && !session.settled) {
                    DropdownMenuItem(
                        text = { Text("Settle session") },
                        onClick = { openMenu = false; actions.settle(session) },
                    )
                }
                DropdownMenuItem(
                    text = { Text("Rename session") },
                    onClick = { openMenu = false; actions.rename(session) },
                )
            }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

@Composable
private fun ProductHeader(
    link: Link,
    title: String,
    onSettings: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface)
            .statusBarsPadding()
            .height(56.dp)
            .padding(start = 16.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            painterResource(R.drawable.ic_xd_mark),
            contentDescription = "xd",
            tint = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.size(22.dp),
        )
        Text(
            title,
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.titleMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        val up = link is Link.Up
        Box(
            modifier = Modifier.size(48.dp),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier
                    .size(9.dp)
                    .clip(CircleShape)
                    .background(
                        if (up) LocalXdSignals.current.working
                        else MaterialTheme.colorScheme.error,
                    ),
            )
        }
        IconButton(onClick = onSettings, modifier = Modifier.size(48.dp)) {
            Icon(
                painter = painterResource(R.drawable.ic_settings),
                contentDescription = "Settings",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

/** Material navigation between the three top-level places. */
@Composable
private fun XdNavigationBar(
    active: MobileDestination,
    onProjects: () -> Unit,
    onSessions: () -> Unit,
    onTerminal: () -> Unit,
    onUsage: () -> Unit,
) {
    NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
        NavItem("Projects", R.drawable.ic_projects, active = active == MobileDestination.PROJECTS, onClick = onProjects)
        NavItem("Sessions", R.drawable.ic_sessions, active = active == MobileDestination.SESSIONS, onClick = onSessions)
        NavItem("Terminal", R.drawable.ic_terminal, active = active == MobileDestination.TERMINAL, onClick = onTerminal)
        NavItem("Usage", R.drawable.ic_usage, active = active == MobileDestination.USAGE, onClick = onUsage)
    }
}

@Composable
private fun XdNavigationRail(
    active: MobileDestination,
    onProjects: () -> Unit,
    onSessions: () -> Unit,
    onTerminal: () -> Unit,
    onUsage: () -> Unit,
) {
    NavigationRail(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Spacer(Modifier.height(16.dp))
        listOf(
            Triple(MobileDestination.PROJECTS, "Projects", R.drawable.ic_projects),
            Triple(MobileDestination.SESSIONS, "Sessions", R.drawable.ic_sessions),
            Triple(MobileDestination.TERMINAL, "Terminal", R.drawable.ic_terminal),
            Triple(MobileDestination.USAGE, "Usage", R.drawable.ic_usage),
        ).forEach { (destination, label, icon) ->
            NavigationRailItem(
                selected = active == destination,
                onClick = when (destination) {
                    MobileDestination.PROJECTS -> onProjects
                    MobileDestination.SESSIONS -> onSessions
                    MobileDestination.TERMINAL -> onTerminal
                    MobileDestination.USAGE -> onUsage
                },
                icon = { Icon(painterResource(icon), contentDescription = null) },
                label = { Text(label) },
            )
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun RowScope.NavItem(label: String, icon: Int, active: Boolean, onClick: () -> Unit) {
    NavigationBarItem(
        selected = active,
        onClick = onClick,
        icon = { Icon(painterResource(icon), contentDescription = null) },
        label = { Text(label) },
        colors = NavigationBarItemDefaults.colors(
            indicatorColor = MaterialTheme.colorScheme.secondaryContainer,
        ),
    )
}

@Composable
private fun ContextBar(title: String, action: (@Composable () -> Unit)? = null) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(52.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .padding(start = 16.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
        action?.invoke()
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

/** What a session's signal tab says about its agent, as on the desktop. */
private enum class SessionSignal(val label: String) {
    IDLE("Idle"),
    WORKING("Working"),
    WAITING("Waiting on you"),
    /** No turn running, but background agents or shells are. */
    BACKGROUND("Waiting on background tasks"),
    DONE("Done"),
}

@Composable
private fun SessionSignal.color(): Color {
    val signals = LocalXdSignals.current
    return when (this) {
        SessionSignal.IDLE -> signals.faint
        SessionSignal.WORKING, SessionSignal.BACKGROUND -> signals.working
        SessionSignal.WAITING -> signals.waiting
        SessionSignal.DONE -> signals.done
    }
}

/**
 * The small status mark at the end of a session row: a dot for work and
 * unread turns, nothing for a quiet session. Waiting on the user is the one
 * state that also says so in words.
 */
@Composable
private fun SignalMark(signal: SessionSignal, modifier: Modifier = Modifier, backgroundTasks: Int = 0) {
    when (signal) {
        SessionSignal.IDLE -> Unit
        SessionSignal.WAITING -> StatusPill("Waiting", signal.color(), modifier)
        SessionSignal.WORKING -> PulseDot(signal.color(), modifier)
        // The count beside a still ring; the words are for screen readers.
        SessionSignal.BACKGROUND -> Row(
            modifier.semantics { contentDescription = backgroundTasksLabel(backgroundTasks) },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text("$backgroundTasks", color = signal.color(), fontSize = 11.sp)
            BackgroundRing(signal.color())
        }
        SessionSignal.DONE -> Box(
            modifier
                .size(7.dp)
                .clip(CircleShape)
                .background(signal.color()),
        )
    }
}

/** A collapsed project: one line, with a dot for each session that needs a look. */
@Composable
private fun DrawerRow(
    project: MinimalProject,
    selected: Boolean,
    signals: List<SessionSignal>,
    onClick: () -> Unit,
    /** The avatar of the account behind the project, in place of the folder icon. */
    icon: ByteArray? = null,
    menu: (@Composable () -> Unit)? = null,
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            painterResource(if (selected) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
            contentDescription = null,
            tint = faint,
            modifier = Modifier.size(14.dp),
        )
        OwnerAvatar(
            icon = icon,
            fallback = R.drawable.ic_folder,
            tint = if (selected) MaterialTheme.colorScheme.onSurfaceVariant else faint,
        )
        Text(
            project.name,
            modifier = Modifier.weight(1f, fill = false),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
            color = if (selected) MaterialTheme.colorScheme.onSurface
            else MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            project.sessions.toString(),
            fontSize = 12.sp,
            color = faint,
        )
        Spacer(Modifier.weight(1f))
        menu?.invoke()
        if (!selected) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                signals.filter { it != SessionSignal.IDLE }.take(5).forEach { signal ->
                    if (signal == SessionSignal.BACKGROUND) {
                        BackgroundRing(signal.color(), size = 6.dp)
                    } else {
                        Box(
                            Modifier
                                .size(6.dp)
                                .clip(CircleShape)
                                .background(signal.color()),
                        )
                    }
                }
            }
        }
    }
}

/** What can be done to a project as a whole, from the "more" button on its open row. */
@Composable
private fun ProjectMenu(
    project: MinimalProject,
    onShortcuts: () -> Unit,
    onRename: () -> Unit,
    /** Null when there is no folder to move it into or out of. */
    onMove: (() -> Unit)?,
    onDelete: () -> Unit,
    onContext: () -> Unit,
    onSecrets: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, modifier = Modifier.size(48.dp)) {
            Icon(
                painterResource(R.drawable.ic_more),
                contentDescription = "${project.name} actions",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(20.dp),
            )
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text("Rename project") },
                onClick = { open = false; onRename() },
            )
            onMove?.let { move ->
                DropdownMenuItem(
                    text = { Text("Move to…") },
                    onClick = { open = false; move() },
                )
            }
            DropdownMenuItem(
                text = { Text("Prompt shortcuts") },
                onClick = { open = false; onShortcuts() },
            )
            DropdownMenuItem(
                text = { Text("Project context") },
                onClick = { open = false; onContext() },
            )
            DropdownMenuItem(
                text = { Text("Agent secrets") },
                onClick = { open = false; onSecrets() },
            )
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Delete project", color = MaterialTheme.colorScheme.error) },
                onClick = { open = false; onDelete() },
            )
        }
    }
}

/** Indent per folder level on the Projects page, in dp. */
private const val FOLDER_INDENT = 16

/** Told when New folder is offered by a host that would make a project instead. */
private const val NEEDS_NEWER_HOST = "Needs a newer xd host"

private fun countLabel(projects: Int, folders: Int): String {
    val projectCount = "$projects project${if (projects == 1) "" else "s"}"
    return if (folders == 0) projectCount else "$projectCount · $folders folder${if (folders == 1) "" else "s"}"
}

/** An account's avatar, or [fallback] until the host has one to show. */
@Composable
private fun OwnerAvatar(icon: ByteArray?, fallback: Int, tint: Color, size: Dp = 16.dp) {
    val bitmap = remember(icon) {
        icon?.let { runCatching { BitmapFactory.decodeByteArray(it, 0, it.size)?.asImageBitmap() }.getOrNull() }
    }
    if (bitmap != null) {
        Image(
            bitmap = bitmap,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(size).clip(RoundedCornerShape(4.dp)),
        )
    } else {
        Icon(painterResource(fallback), contentDescription = null, tint = tint, modifier = Modifier.size(size))
    }
}

/** A menu entry, with a line saying why it cannot be used when it cannot. */
@Composable
private fun MenuLabel(label: String, detail: String? = null) {
    Column {
        Text(label)
        detail?.let {
            Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** The page's New button: a project or a folder, as the desktop's + offers. */
@Composable
private fun NewMenu(canCreateFolders: Boolean, onProject: () -> Unit, onFolder: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { open = true }) {
            Icon(painterResource(R.drawable.ic_add), contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
            Text("New")
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text("New project") },
                onClick = { open = false; onProject() },
            )
            DropdownMenuItem(
                text = { MenuLabel("New folder", NEEDS_NEWER_HOST.takeUnless { canCreateFolders }) },
                enabled = canCreateFolders,
                onClick = { open = false; onFolder() },
            )
        }
    }
}

/** A folder of projects: one line that opens and closes what it holds. */
@Composable
private fun FolderRow(
    folder: MinimalFolder,
    open: Boolean,
    icon: ByteArray?,
    toggle: () -> Unit,
    modifier: Modifier = Modifier,
    menu: @Composable () -> Unit,
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClickLabel = if (open) "Close folder" else "Open folder", onClick = toggle)
            .padding(start = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(
            painterResource(if (open) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
            contentDescription = null,
            tint = faint,
            modifier = Modifier.size(14.dp),
        )
        OwnerAvatar(icon = icon, fallback = R.drawable.ic_folders, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            folder.name,
            modifier = Modifier.weight(1f, fill = false),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(folder.items.toString(), fontSize = 12.sp, color = faint)
        Spacer(Modifier.weight(1f))
        menu()
    }
}

/** What can be done with a folder, as its right-click menu offers on the desktop. */
@Composable
private fun FolderMenu(
    folder: MinimalFolder,
    canCreateFolders: Boolean,
    onNewProject: () -> Unit,
    onNewFolder: () -> Unit,
    /** Null when there is no other folder to move it into or out of. */
    onMove: (() -> Unit)?,
    onRename: () -> Unit,
    /** Shortcuts, context, and secrets set here reach every project inside. */
    onShortcuts: () -> Unit,
    onContext: () -> Unit,
    onSecrets: () -> Unit,
    onDelete: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, modifier = Modifier.size(48.dp)) {
            Icon(
                painterResource(R.drawable.ic_more),
                contentDescription = "${folder.name} actions",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(20.dp),
            )
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { Text("New project here") },
                onClick = { open = false; onNewProject() },
            )
            DropdownMenuItem(
                text = { MenuLabel("New folder here", NEEDS_NEWER_HOST.takeUnless { canCreateFolders }) },
                enabled = canCreateFolders,
                onClick = { open = false; onNewFolder() },
            )
            onMove?.let { move ->
                DropdownMenuItem(
                    text = { Text("Move to…") },
                    onClick = { open = false; move() },
                )
            }
            DropdownMenuItem(
                text = { Text("Rename folder") },
                onClick = { open = false; onRename() },
            )
            DropdownMenuItem(
                text = { Text("Prompt shortcuts") },
                onClick = { open = false; onShortcuts() },
            )
            DropdownMenuItem(
                text = { Text("Folder context") },
                onClick = { open = false; onContext() },
            )
            DropdownMenuItem(
                text = { Text("Agent secrets") },
                onClick = { open = false; onSecrets() },
            )
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Delete folder", color = MaterialTheme.colorScheme.error) },
                onClick = { open = false; onDelete() },
            )
        }
    }
}

/** Names a new folder of projects, optionally after a GitHub organization whose avatar it shows. */
@Composable
private fun NewFolderDialog(
    /** The folder it is made in; null for the top level. */
    parentName: String?,
    /** An earlier creation is still out; Create waits for it. */
    busy: Boolean,
    onDismiss: () -> Unit,
    onCreate: (name: String, owner: String?) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf("") }
    var organization by rememberSaveable { mutableStateOf("") }
    val login = organizationLogin(organization)
    val organizationError = organization.isNotBlank() && login == null
    // A folder for an organization is named after it unless named otherwise.
    val folderName = name.trim().ifEmpty { login.orEmpty() }
    val nameError = folderNameError(folderName)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New folder") },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                parentName?.let { Text("In $it", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Name") },
                    singleLine = true,
                    isError = nameError != null,
                    supportingText = nameError?.let { { Text(it) } },
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = organization,
                    onValueChange = { organization = it },
                    label = { Text("GitHub organization (optional)") },
                    placeholder = { Text("acme or github.com/acme") },
                    singleLine = true,
                    isError = organizationError,
                    supportingText = {
                        Text(
                            if (organizationError) "Enter a GitHub organization name, such as acme or github.com/acme."
                            else "Shows its avatar. Leave the name empty to use the organization's.",
                        )
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                Text(
                    "Folders hold projects and other folders. Use Move to on a project to put it in one.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
        confirmButton = {
            Button(
                onClick = { onCreate(folderName, login) },
                enabled = folderName.isNotEmpty() && nameError == null && !organizationError && !busy,
            ) { Text("Create folder") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Picks the folder a project or folder moves into, or the top level. */
@Composable
private fun MoveDialog(
    name: String,
    /** The folder it is in now; null at the top level. */
    current: String?,
    destinations: List<FolderDestination>,
    icons: Map<String, ByteArray?>,
    onDismiss: () -> Unit,
    onMove: (parentId: String?) -> Unit,
) {
    // Saved as "" for the top level, which a saved null could not tell from nothing restored.
    var choice by rememberSaveable { mutableStateOf(current.orEmpty()) }
    val chosen = choice.ifEmpty { null }
    val offered = chosen == null || destinations.any { it.folder.id == chosen }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Move $name", maxLines = 1, overflow = TextOverflow.Ellipsis) },
        text = {
            Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                MoveRow(
                    label = "Top level",
                    depth = 0,
                    icon = null,
                    fallback = R.drawable.ic_projects,
                    chosen = chosen == null,
                    here = current == null,
                    onClick = { choice = "" },
                )
                destinations.forEach { destination ->
                    MoveRow(
                        label = destination.folder.name,
                        depth = destination.depth + 1,
                        icon = icons[destination.folder.owner],
                        fallback = R.drawable.ic_folders,
                        chosen = chosen == destination.folder.id,
                        here = current == destination.folder.id,
                        onClick = { choice = destination.folder.id },
                    )
                }
            }
        },
        confirmButton = {
            Button(onClick = { onMove(chosen) }, enabled = offered && chosen != current) { Text("Move") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun MoveRow(
    label: String,
    depth: Int,
    icon: ByteArray?,
    fallback: Int,
    chosen: Boolean,
    /** It is already here. */
    here: Boolean,
    onClick: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.RadioButton, onClick = onClick)
            .padding(start = (depth * 12).dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        RadioButton(selected = chosen, onClick = null)
        OwnerAvatar(icon = icon, fallback = fallback, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            label,
            modifier = Modifier.weight(1f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodyMedium,
        )
        if (here) {
            Text("Here now", color = LocalXdSignals.current.faint, style = MaterialTheme.typography.labelSmall)
        }
    }
}

/** The current project, open, with one row per session beneath it. */
@Composable
private fun OpenDrawer(
    project: MinimalProject,
    icon: ByteArray?,
    sessions: List<MinimalSession>,
    signalOf: (MinimalSession) -> SessionSignal,
    open: (MinimalSession) -> Unit,
    actions: SessionActions,
    menu: @Composable () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        DrawerRow(project = project, selected = true, signals = emptyList(), onClick = {}, icon = icon, menu = menu)
        if (sessions.isEmpty()) {
            Text(
                "No sessions yet. Start one with Codex, Claude, or JCode.",
                modifier = Modifier.padding(start = 30.dp, end = 10.dp, top = 4.dp, bottom = 8.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        sessions.forEach { session ->
            // Keyed so a row's open menu stays with its session when others come and go.
            key(session.id) {
                SessionRow(session = session, signal = signalOf(session), onClick = { open(session) }, actions = actions)
            }
        }
    }
}

/**
 * One session, on one line: its agent, its title, and a status mark only when
 * there is something to say. A branch shows when the session has its own.
 */
@Composable
private fun SessionRow(
    session: MinimalSession,
    signal: SessionSignal,
    onClick: () -> Unit,
    actions: SessionActions,
    subtitle: String? = null,
) {
    val signals = LocalXdSignals.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 18.dp)
            .heightIn(min = 64.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(start = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        BackendIcon(session.agent.wire, size = 15.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                session.title,
                style = MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    subtitle ?: "${session.chatIds.size} chat${if (session.chatIds.size == 1) "" else "s"}",
                    style = MaterialTheme.typography.labelMedium,
                    color = signals.faint,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (session.branch != "Project directory") Text(
                    session.branch,
                    modifier = Modifier.widthIn(max = 160.dp),
                    color = signals.faint,
                    fontFamily = XdMonoFamily,
                    style = MaterialTheme.typography.labelMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        SignalMark(signal, backgroundTasks = session.backgroundTasks)
        if (session.settled) {
            IconButton(onClick = { actions.unsettle(session) }, modifier = Modifier.size(48.dp)) {
                Icon(
                    painterResource(R.drawable.ic_undo),
                    contentDescription = "Bring back ${session.title}",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(18.dp),
                )
            }
        }
        SessionMenu(session, actions)
    }
}

/**
 * What can be done to one session, as in the desktop's right-click menu.
 * Settling is offered only when the host supports it.
 */
@Composable
private fun SessionMenu(session: MinimalSession, actions: SessionActions) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, modifier = Modifier.size(48.dp)) {
            Icon(
                painterResource(R.drawable.ic_more),
                contentDescription = "${session.title} actions",
                tint = LocalXdSignals.current.faint,
                modifier = Modifier.size(18.dp),
            )
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            if (actions.settlement) {
                if (session.settled) {
                    DropdownMenuItem(
                        text = { Text("Bring back") },
                        onClick = { open = false; actions.unsettle(session) },
                    )
                } else {
                    DropdownMenuItem(
                        text = { Text("Settle session") },
                        onClick = { open = false; actions.settle(session) },
                    )
                }
            }
            DropdownMenuItem(
                text = { Text("Rename session") },
                onClick = { open = false; actions.rename(session) },
            )
            if (actions.settlement) {
                DropdownMenuItem(
                    text = { Text(if (session.autoSettleDisabled) "Enable auto-settle" else "Disable auto-settle") },
                    onClick = { open = false; actions.setAutoSettle(session, session.autoSettleDisabled) },
                )
            }
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Delete session", color = MaterialTheme.colorScheme.error) },
                onClick = { open = false; actions.delete(session) },
            )
        }
    }
}

/** What the app's session menus can do; built once at the top so every surface agrees. */
private class SessionActions(
    val settlement: Boolean,
    val settle: (MinimalSession) -> Unit,
    val unsettle: (MinimalSession) -> Unit,
    val setAutoSettle: (MinimalSession, Boolean) -> Unit,
    val rename: (MinimalSession) -> Unit,
    val delete: (MinimalSession) -> Unit,
)

/**
 * Settled sessions, below the projects as on the desktop: out of the way, most
 * recently settled first, each a tap from coming back.
 */
@Composable
private fun SettledShelf(
    settled: List<SettledSession>,
    expanded: Boolean,
    toggle: () -> Unit,
    open: (SettledSession) -> Unit,
    actions: SessionActions,
) {
    val faint = LocalXdSignals.current.faint
    Column(Modifier.fillMaxWidth().padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 48.dp)
                .clip(RoundedCornerShape(10.dp))
                .clickable(onClick = toggle)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(
                painterResource(if (expanded) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
                contentDescription = null,
                tint = faint,
                modifier = Modifier.size(14.dp),
            )
            Icon(painterResource(R.drawable.ic_check), contentDescription = null, tint = faint, modifier = Modifier.size(15.dp))
            Text(
                "Settled",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(settled.size.toString(), fontSize = 12.sp, color = faint)
        }
        if (expanded) {
            settled.forEach { entry ->
                key(entry.session.id) {
                    SessionRow(
                        session = entry.session,
                        signal = SessionSignal.IDLE,
                        onClick = { open(entry) },
                        actions = actions,
                        subtitle = entry.project.name,
                    )
                }
            }
        }
    }
}

/** A small status label on a soft tint of its own colour. */
@Composable
internal fun StatusPill(label: String, color: Color, modifier: Modifier = Modifier) {
    Text(
        label,
        modifier = modifier
            .clip(RoundedCornerShape(50))
            .background(color.copy(alpha = 0.14f))
            .padding(horizontal = 8.dp, vertical = 2.dp),
        color = color,
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.Medium,
    )
}

/** A dot that breathes while something works. */
@Composable
internal fun PulseDot(color: Color, modifier: Modifier = Modifier, size: Dp = 7.dp) {
    val transition = androidx.compose.animation.core.rememberInfiniteTransition(label = "pulse")
    val alpha by transition.animateFloat(
        initialValue = 0.35f,
        targetValue = 1f,
        animationSpec = androidx.compose.animation.core.infiniteRepeatable(
            androidx.compose.animation.core.tween(800),
            androidx.compose.animation.core.RepeatMode.Reverse,
        ),
        label = "pulse-alpha",
    )
    Box(
        modifier
            .size(size)
            .graphicsLayer { this.alpha = alpha }
            .clip(CircleShape)
            .background(color),
    )
}

@Composable
private fun AgentPill(agent: DirectAgent) {
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.surfaceContainerHigh)
            .padding(horizontal = 10.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        BackendIcon(agent.wire, size = 15.dp)
        Text(agent.wire, fontFamily = XdMonoFamily, fontSize = 11.sp)
    }
}

private class NativeChatOwner : ViewModelStoreOwner {
    override val viewModelStore: ViewModelStore = ViewModelStore()
}

@Composable
private fun EmptyCard(text: String) {
    Text(
        text,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(14.dp))
            .padding(20.dp),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
internal fun NewSessionDialog(
    projectName: String,
    draftKey: String? = null,
    backends: List<com.restartfu.xd.protocol.BackendReply> = emptyList(),
    catalogError: String? = null,
    retryCatalog: (() -> Unit)? = null,
    chatTab: Boolean = false,
    /** Where a new session can work; null for a chat joining an existing session. */
    places: (suspend () -> NewSessionPlaces)? = null,
    onDismiss: () -> Unit,
    onCreate: (String, DirectAgent, Ops.NewSessionWorktree?, String?, String?, Boolean?) -> Unit,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val drafts = remember(context) { context.getSharedPreferences("new-chat-drafts", android.content.Context.MODE_PRIVATE) }
    val restored = remember(draftKey) { runCatching { org.json.JSONObject(draftKey?.let { drafts.getString(it, null) } ?: "{}") }.getOrDefault(org.json.JSONObject()) }
    var title by rememberSaveable(draftKey) { mutableStateOf(restored.optString("title")) }
    var agent by rememberSaveable(draftKey) { mutableStateOf(DirectAgent.entries.firstOrNull { it.wire == restored.optString("agent") } ?: DirectAgent.CODEX) }
    var effort by rememberSaveable(draftKey) { mutableStateOf(restored.optString("effort").takeIf(String::isNotBlank)) }
    var access by rememberSaveable(draftKey) { mutableStateOf(restored.optString("access").takeIf(String::isNotBlank)) }
    var plan by rememberSaveable(draftKey) { mutableStateOf(restored.optBoolean("plan")) }
    var loaded by remember { mutableStateOf<NewSessionPlaces?>(null) }
    var placesError by remember { mutableStateOf<String?>(null) }
    var worktree by rememberSaveable(draftKey, stateSaver = WorktreeChoiceSaver) {
        mutableStateOf<Ops.NewSessionWorktree?>(runCatching {
            val saved = restored.optJSONObject("worktree") ?: return@runCatching null
            when (saved.optString("kind")) {
                "new" -> Ops.NewSessionWorktree.New
                "existing" -> Ops.NewSessionWorktree.Existing(saved.getString("path"))
                "branch" -> Ops.NewSessionWorktree.Branch(saved.getString("ref"), saved.getString("oid"))
                "pull-request" -> Ops.NewSessionWorktree.PullRequest(saved.getLong("number"), saved.getString("oid"))
                else -> null
            }
        }.getOrNull())
    }
    androidx.compose.runtime.LaunchedEffect(draftKey, title, agent, effort, access, plan, worktree) {
        draftKey?.let { key ->
            drafts.edit().putString(key, org.json.JSONObject().put("title", title).put("agent", agent.wire)
                .put("effort", effort).put("access", access).put("plan", plan)
                .put("worktree", worktree?.let { choice ->
                    org.json.JSONObject().apply {
                        when (choice) {
                            Ops.NewSessionWorktree.New -> put("kind", "new")
                            is Ops.NewSessionWorktree.Existing -> { put("kind", "existing"); put("path", choice.path) }
                            is Ops.NewSessionWorktree.Branch -> { put("kind", "branch"); put("ref", choice.reference); put("oid", choice.oid) }
                            is Ops.NewSessionWorktree.PullRequest -> { put("kind", "pull-request"); put("number", choice.number); put("oid", choice.oid) }
                        }
                    }
                }).toString()).apply()
        }
    }
    if (places != null) {
        LaunchedEffect(Unit) {
            try {
                val found = places()
                loaded = found
                worktree = found.reconcile(worktree)
            } catch (error: kotlinx.coroutines.CancellationException) {
                throw error
            } catch (error: Throwable) {
                // The host still picks the project's own checkout, so a choice
                // restored from before a rotation is dropped rather than sent unseen.
                worktree = null
                placesError = error.message ?: "Could not list where a session can work"
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (chatTab) "New chat" else "New session") },
        text = {
            Column(
                modifier = Modifier.fillMaxWidth().heightIn(max = 420.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(projectName, color = MaterialTheme.colorScheme.onSurfaceVariant)
                OutlinedTextField(
                    value = title,
                    onValueChange = { title = it },
                    label = { Text("What are you working on?") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    DirectAgent.entries.forEach { candidate ->
                        FilterChip(
                            selected = agent == candidate,
                            onClick = { agent = candidate; effort = null },
                            label = { Text(candidate.label) },
                            leadingIcon = { BackendIcon(candidate.wire, size = 16.dp) },
                        )
                    }
                }
                if (retryCatalog != null && backends.isEmpty()) {
                    Text(catalogError ?: "Loading assistant options…", style = MaterialTheme.typography.bodySmall)
                    if (catalogError != null) TextButton(onClick = retryCatalog) { Text("Retry") }
                }
                val efforts = backends.firstOrNull { it.id == agent.wire }?.efforts.orEmpty()
                if (efforts.isNotEmpty()) CreationOption("Reasoning effort", effort ?: "Default", listOf("Default") + efforts) { effort = it.takeUnless { it == "Default" } }
                CreationOption("Workspace access", if (plan) "Read only" else when (access) { "read-only" -> "Read only"; "edit" -> "Edit"; "full" -> "Full access"; else -> "Default" },
                    listOf("Default", "Read only", "Edit", "Full access"), enabled = !plan) {
                    access = when (it) { "Read only" -> "read-only"; "Edit" -> "edit"; "Full access" -> "full"; else -> null }
                }
                CreationOption("Chat mode", if (plan) "Plan" else "Build", listOf("Build", "Plan")) { plan = it == "Plan" }
                if (places != null) {
                    WorkspacePicker(
                        places = loaded,
                        error = placesError,
                        selected = worktree,
                        onSelect = { worktree = it },
                    )
                }
            }
        },
        confirmButton = {
            Button(
                onClick = { onCreate(title.trim(), agent, worktree, effort?.takeIf { it in backends.firstOrNull { b -> b.id == agent.wire }?.efforts.orEmpty() }, access, plan) },
                // Wait for the choices, as the desktop does; if they cannot be
                // read, the host's own default checkout still works.
                // Only once this opening's choices have loaded, even if a choice
                // was restored from before a rotation; a project with no
                // checkout to list leaves the choice to the host, as before.
                enabled = (retryCatalog == null || backends.isNotEmpty()) &&
                    (places == null || placesError != null ||
                    (loaded != null && (worktree != null || loaded?.default == null))),
            ) {
                Text(if (chatTab) "Create chat" else "Create session")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun CreationOption(label: String, value: String, choices: List<String>, enabled: Boolean = true, onSelect: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, style = MaterialTheme.typography.bodyMedium)
        Box {
            TextButton(onClick = { open = true }, enabled = enabled) { Text(value) }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                choices.forEach { choice -> DropdownMenuItem(text = { Text(choice) }, onClick = { open = false; onSelect(choice) }) }
            }
        }
    }
}

/** Where a new session in a project can work, as the desktop's new session dialog offers. */
internal data class NewSessionPlaces(
    val worktrees: List<WorktreeReply>,
    val canCreateWorktree: Boolean,
    val branches: List<BranchSourceReply>,
    val pullRequests: List<PullRequestSourceReply>,
    val pullRequestError: String?,
    /** Why branches and pull requests could not be listed at all. */
    val sourcesError: String? = null,
) {
    /**
     * A choice restored from before a rotation, matched against what the host
     * lists now: a branch or pull request keeps its identity with its current
     * commit, and anything no longer offered falls back to the default.
     */
    fun reconcile(choice: Ops.NewSessionWorktree?): Ops.NewSessionWorktree? = when (choice) {
        null -> default
        Ops.NewSessionWorktree.New -> if (canCreateWorktree) choice else default
        is Ops.NewSessionWorktree.Existing -> if (worktrees.any { it.path == choice.path }) choice else default
        // A failed listing says nothing about whether the choice still exists,
        // so it is kept, and the host checks its commit again on creating.
        is Ops.NewSessionWorktree.Branch -> if (sourcesError != null) choice else
            branches.firstOrNull { it.reference == choice.reference }
                ?.let { Ops.NewSessionWorktree.Branch(it.reference, it.oid) } ?: default
        // Pull requests can fail to list on their own, while branches still list.
        is Ops.NewSessionWorktree.PullRequest -> if (sourcesError != null || pullRequestError != null) choice else
            pullRequests.firstOrNull { it.number == choice.number }
                ?.let { Ops.NewSessionWorktree.PullRequest(it.number, it.oid) } ?: default
    }

    /** The checkout in use, else the project's own, as the desktop picks. */
    val default: Ops.NewSessionWorktree?
        get() = (worktrees.firstOrNull { it.current } ?: worktrees.firstOrNull { it.main } ?: worktrees.firstOrNull())
            ?.let { Ops.NewSessionWorktree.Existing(it.path) }

    companion object {
        suspend fun load(client: com.restartfu.xd.XdClient, folderId: String): NewSessionPlaces = coroutineScope {
            val settings = async { client.folderSettings(folderId) }
            // Branches and pull requests are extras: a project without Git, or a
            // host without them, still offers its checkouts.
            val sources = async { runCatching { client.newChatSources(folderId) } }
            val folder = settings.await()
            val listed = sources.await()
            val found = listed.getOrNull()
            val worktrees = folder.worktrees.ifEmpty {
                listOfNotNull(folder.effectiveWorkdir?.takeIf(String::isNotBlank)?.let {
                    WorktreeReply(path = it, detached = false, main = true, current = true)
                })
            }
            NewSessionPlaces(
                worktrees = worktrees,
                canCreateWorktree = folder.canCreateWorktree,
                branches = found?.branches.orEmpty(),
                pullRequests = found?.pullRequests.orEmpty(),
                pullRequestError = found?.pullRequestError,
                sourcesError = listed.exceptionOrNull()?.let { it.message ?: "Could not list branches and pull requests" },
            )
        }
    }
}

/**
 * A checkout, a new worktree, a branch, or an open pull request. Branches and
 * pull requests get their own managed worktree on the host.
 */
@Composable
private fun WorkspacePicker(
    places: NewSessionPlaces?,
    error: String?,
    selected: Ops.NewSessionWorktree?,
    onSelect: (Ops.NewSessionWorktree) -> Unit,
) {
    var filter by rememberSaveable { mutableStateOf("") }
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text("Workspace", style = MaterialTheme.typography.titleSmall)
        when {
            error != null -> Text(
                "$error The project's own checkout will be used.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
            places == null -> Text(
                "Reading the project's checkouts…",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
            else -> {
                // New worktree comes first, as on the desktop.
                if (places.canCreateWorktree) {
                    PlaceRow(
                        label = "New worktree",
                        detail = "A fresh branch in its own checkout",
                        chosen = selected == Ops.NewSessionWorktree.New,
                        onClick = { onSelect(Ops.NewSessionWorktree.New) },
                    )
                }
                places.worktrees.forEach { checkout ->
                    PlaceRow(
                        label = checkout.branch ?: checkout.path.substringAfterLast('/'),
                        detail = if (checkout.main) "Project directory" else checkout.path,
                        chosen = selected == Ops.NewSessionWorktree.Existing(checkout.path),
                        onClick = { onSelect(Ops.NewSessionWorktree.Existing(checkout.path)) },
                    )
                }
                if (places.branches.isNotEmpty() || places.pullRequests.isNotEmpty()) {
                    OutlinedTextField(
                        value = filter,
                        onValueChange = { filter = it },
                        label = { Text("Find a branch or pull request") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
                    )
                    val query = filter.trim()
                    // The chosen one always shows, whatever the filter and limit hide.
                    places.pullRequests
                        .filter { query.isEmpty() || it.title.contains(query, true) || "#${it.number}".contains(query) }
                        .take(PLACE_LIMIT)
                        .let { shown ->
                            val chosen = (selected as? Ops.NewSessionWorktree.PullRequest)?.number
                            shown + places.pullRequests.filter { it.number == chosen && it !in shown }
                        }
                        .forEach { pull ->
                            PlaceRow(
                                label = "#${pull.number} ${pull.title}",
                                detail = "Pull request",
                                chosen = selected == Ops.NewSessionWorktree.PullRequest(pull.number, pull.oid),
                                onClick = { onSelect(Ops.NewSessionWorktree.PullRequest(pull.number, pull.oid)) },
                            )
                        }
                    places.branches
                        .filter { query.isEmpty() || it.name.contains(query, true) }
                        .take(PLACE_LIMIT)
                        .let { shown ->
                            val chosen = (selected as? Ops.NewSessionWorktree.Branch)?.reference
                            shown + places.branches.filter { it.reference == chosen && it !in shown }
                        }
                        .forEach { branch ->
                            PlaceRow(
                                label = branch.name,
                                detail = "Branch",
                                chosen = selected == Ops.NewSessionWorktree.Branch(branch.reference, branch.oid),
                                onClick = { onSelect(Ops.NewSessionWorktree.Branch(branch.reference, branch.oid)) },
                                mono = true,
                            )
                        }
                }
                // A choice kept through a failed listing still shows, so Create
                // never uses something the picker does not show.
                when (val kept = selected) {
                    is Ops.NewSessionWorktree.PullRequest -> if (places.pullRequests.none { it.number == kept.number }) {
                        PlaceRow(
                            label = "#${kept.number}",
                            detail = "Pull request, chosen before it could be listed",
                            chosen = true,
                            onClick = {},
                        )
                    }
                    is Ops.NewSessionWorktree.Branch -> if (places.branches.none { it.reference == kept.reference }) {
                        PlaceRow(
                            label = kept.reference.removePrefix("refs/heads/").removePrefix("refs/remotes/"),
                            detail = "Branch, chosen before it could be listed",
                            chosen = true,
                            onClick = {},
                            mono = true,
                        )
                    }
                    else -> Unit
                }
                places.sourcesError?.let {
                    Text(
                        "Branches and pull requests could not be listed: $it",
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                places.pullRequestError?.let {
                    Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** Keeps a workspace choice across rotation. */
private val WorktreeChoiceSaver = listSaver<Ops.NewSessionWorktree?, String>(
    save = { choice ->
        when (choice) {
            null -> emptyList()
            Ops.NewSessionWorktree.New -> listOf("new")
            is Ops.NewSessionWorktree.Existing -> listOf("existing", choice.path)
            is Ops.NewSessionWorktree.Branch -> listOf("branch", choice.reference, choice.oid)
            is Ops.NewSessionWorktree.PullRequest -> listOf("pull-request", choice.number.toString(), choice.oid)
        }
    },
    restore = { saved ->
        when (saved.firstOrNull()) {
            "new" -> Ops.NewSessionWorktree.New
            "existing" -> Ops.NewSessionWorktree.Existing(saved[1])
            "branch" -> Ops.NewSessionWorktree.Branch(saved[1], saved[2])
            "pull-request" -> Ops.NewSessionWorktree.PullRequest(saved[1].toLong(), saved[2])
            else -> null
        }
    },
)

/** Branches and pull requests shown at once; the filter finds the rest. */
private const val PLACE_LIMIT = 12

@Composable
private fun PlaceRow(label: String, detail: String, chosen: Boolean, onClick: () -> Unit, mono: Boolean = false) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        RadioButton(selected = chosen, onClick = null)
        Column(Modifier.weight(1f)) {
            Text(
                label,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                fontFamily = if (mono) XdMonoFamily else null,
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                detail,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.labelSmall,
            )
        }
    }
}

@Composable
private fun NewProjectDialog(
    model: MainViewModel,
    /** The folder it is made in; null for the top level. */
    parentName: String? = null,
    /** An earlier creation is still out; Create waits for it. */
    busy: Boolean = false,
    onDismiss: () -> Unit,
    onCreate: (String, String?) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf("") }
    var repository by rememberSaveable { mutableStateOf("") }
    var browsing by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New project") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                parentName?.let { Text("In $it", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Name") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = repository,
                    onValueChange = { repository = it },
                    label = { Text("Repository path (optional)") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                    trailingIcon = {
                        IconButton(onClick = { browsing = true }) {
                            Icon(
                                painterResource(R.drawable.ic_folder),
                                contentDescription = "Browse folders on the host",
                                modifier = Modifier.size(20.dp),
                            )
                        }
                    },
                )
            }
        },
        confirmButton = {
            Button(
                // Sent exactly as chosen: a Unix folder name can end in a space.
                onClick = { onCreate(name.trim(), repository.takeIf(String::isNotBlank)) },
                enabled = name.isNotBlank() && !busy,
            ) { Text("Create project") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
    if (browsing) {
        RepositoryPickerDialog(
            listDirectories = model.client::listDirectories,
            initialPath = repository,
            onDismiss = { browsing = false },
            onChoose = { path ->
                browsing = false
                repository = path
                // A repository usually names its project; keep any name already typed.
                if (name.isBlank()) name = path.trimEnd('/').substringAfterLast('/')
            },
        )
    }
}

/**
 * The app's settings: how it looks and speaks on this phone, and the parts of
 * the connected host it can manage, as on the desktop's Settings page.
 */
@Composable
private fun MinimalSettingsDialog(
    model: MainViewModel,
    settings: MobileSettings,
    onDismiss: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val theme by settings.theme.collectAsStateWithLifecycle()
    val allPermissions by settings.allowAllPermissions.collectAsStateWithLifecycle()
    val speech by settings.speechEnabled.collectAsStateWithLifecycle()
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val resumeChats by settings.resumeChats.collectAsStateWithLifecycle()
    var updatingHost by rememberSaveable { mutableStateOf(false) }
    var accountsOpen by rememberSaveable { mutableStateOf(false) }
    var secretsOpen by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Text("Appearance", style = MaterialTheme.typography.titleSmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    MinimalThemePreset.entries.forEach { preset ->
                        val chosen = theme == preset
                        Row(
                            modifier = Modifier
                                .weight(1f)
                                .heightIn(min = 48.dp)
                                .clip(RoundedCornerShape(8.dp))
                                .border(
                                    if (chosen) 1.5.dp else 1.dp,
                                    if (chosen) MaterialTheme.colorScheme.primary
                                    else MaterialTheme.colorScheme.outlineVariant,
                                    RoundedCornerShape(8.dp),
                                )
                                .clickable { settings.setTheme(preset) }
                                .padding(horizontal = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Box(
                                Modifier
                                    .size(width = 22.dp, height = 16.dp)
                                    .clip(RoundedCornerShape(3.dp))
                                    .background(preset.preview)
                                    .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(3.dp)),
                            )
                            Text(
                                preset.label,
                                fontWeight = if (chosen) FontWeight.SemiBold else FontWeight.Normal,
                            )
                        }
                    }
                }
                HorizontalDivider()
                SettingSwitch(
                    title = "Read replies aloud",
                    detail = "Speaks the parts of a reply the agent marks with <speak> tags.",
                    checked = speech,
                    onCheckedChange = settings::setSpeechEnabled,
                )
                SettingSwitch(
                    title = "All permissions",
                    detail = "Pass the agent's unrestricted command-line flag.",
                    checked = allPermissions,
                    onCheckedChange = settings::setAllowAllPermissions,
                )
                SettingSwitch(
                    title = "Resume chats after a restart",
                    detail = "Continue turns a restart cut off. Agents start working without asking.",
                    checked = resumeChats,
                    onCheckedChange = settings::setResumeChats,
                )
                HorizontalDivider()
                Text("Host", style = MaterialTheme.typography.titleSmall)
                SettingAction(
                    title = "Prompt shortcuts",
                    detail = "Buttons above the composer in every project on this host.",
                    action = "Edit",
                ) {
                    onDismiss()
                    model.openShortcutEditor(null, "Prompt shortcuts")
                }
                ChatTitleSettings(model)
                if (tree.settlement) AutoSettleSettings(model)
                SettingAction(
                    title = "Assistant accounts",
                    detail = "Manage sign-in and the installed assistant CLIs.",
                    action = "Open",
                ) { accountsOpen = true }
                SettingAction(
                    title = "Agent secrets",
                    detail = "Environment variables the host gives every agent.",
                    action = "Edit",
                ) { secretsOpen = true }
                SettingAction(
                    title = "Host updates",
                    detail = "Check for a new xd build on the connected machine.",
                    action = "Check",
                ) { updatingHost = true }
                OutlinedButton(onClick = onDisconnect, modifier = Modifier.fillMaxWidth()) {
                    Text("Disconnect remote")
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
    if (updatingHost) {
        HostUpdateDialog(model = model, onDismiss = { updatingHost = false })
    }
    if (accountsOpen) {
        AssistantAccountsDialog(model = model, onDismiss = { accountsOpen = false })
    }
    if (secretsOpen) {
        AgentSecretsDialog(model = model, folderId = null, title = "Agent secrets", onDismiss = { secretsOpen = false })
    }
}

/**
 * The host's rules for settling sessions by itself, with the desktop's
 * wording. They belong to the host, so they keep running while no client is
 * connected.
 */
@Composable
private fun AutoSettleSettings(model: MainViewModel) {
    val rules by model.autoSettle.collectAsStateWithLifecycle()
    val readError by model.autoSettleError.collectAsStateWithLifecycle()
    val writeError by model.autoSettleWriteError.collectAsStateWithLifecycle()
    // A failed change stays shown even after a read brings the rules back.
    val error = writeError ?: readError
    LaunchedEffect(Unit) { model.refreshAutoSettle() }
    val current = rules
    // Edits wait until no request is out, so none is computed from rules that
    // are about to change and no tap is lost.
    val busy by model.autoSettleBusy.collectAsStateWithLifecycle()
    val stale by model.autoSettleStale.collectAsStateWithLifecycle()
    // Nor while the rules shown are known to be out of date: a relative
    // change such as one more day must start from the host's value.
    val editable = busy == 0 && !stale
    if (current == null) {
        Text(
            error ?: "Reading auto-settle rules…",
            color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        // Also while reading: a read started by a broadcast fails quietly.
        TextButton(onClick = model::loadAutoSettle) { Text("Try again") }
        return
    }
    SettingSwitch(
        title = "Auto-settle merged sessions",
        detail = "Settle a session when its pull request merges. A closed pull request settles it while either rule here is on.",
        checked = current.onMerge,
        onCheckedChange = { model.setAutoSettle(onMerge = it) },
        enabled = editable,
    )
    SettingSwitch(
        title = "Auto-settle inactive sessions",
        detail = "Sessions with no activity for this long settle automatically.",
        checked = current.afterDays != null,
        onCheckedChange = { on ->
            model.setAutoSettle(afterDays = if (on) Ops.AfterDays.Days(DEFAULT_SETTLE_DAYS) else Ops.AfterDays.Off)
        },
        enabled = editable,
    )
    val setDays = { days: Int -> model.setAutoSettle(afterDays = Ops.AfterDays.Days(days)) }
    current.afterDays?.let { days ->
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = 8.dp)) {
                Text("Days before auto-settle", fontWeight = FontWeight.SemiBold)
                Text(
                    "Any new activity un-settles a session automatically.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            IconButton(
                onClick = { setDays(days - 1) },
                enabled = editable && days > 1,
            ) { Text("\u2212", style = MaterialTheme.typography.titleMedium) }
            Text(days.toString(), fontWeight = FontWeight.SemiBold, modifier = Modifier.widthIn(min = 24.dp))
            IconButton(
                onClick = { setDays(days + 1) },
                enabled = editable && days < 90,
            ) { Text("+", style = MaterialTheme.typography.titleMedium) }
        }
    }
    SettingSwitch(
        title = "Keep sessions with a PR",
        detail = "Inactivity does not settle sessions with a pull request. Merge and close still do.",
        checked = current.keepPullRequests,
        onCheckedChange = { model.setAutoSettle(keepPullRequests = it) },
        enabled = editable,
    )
    // Any failure keeps Try again in reach, including a failed retry.
    if (stale || error != null) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                error ?: "These rules changed on the host and could not be read again.",
                modifier = Modifier.weight(1f),
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
            TextButton(onClick = model::loadAutoSettle) { Text("Try again") }
        }
    }
}

/** The host's own default for the inactivity rule. */
private const val DEFAULT_SETTLE_DAYS = 3

@Composable
private fun SettingSwitch(
    title: String,
    detail: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    enabled: Boolean = true,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, fontWeight = FontWeight.SemiBold)
            Text(
                detail,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange, enabled = enabled)
    }
}

@Composable
private fun SettingAction(
    title: String,
    detail: String,
    action: String,
    onClick: () -> Unit,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, fontWeight = FontWeight.SemiBold)
            Text(
                detail,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        TextButton(onClick = onClick) { Text(action) }
    }
}

/** Asks for a new name, keeping the old one until it is changed. */
@Composable
private fun NameDialog(
    title: String,
    /** What is being renamed; a draft saved for anything else is not restored. */
    target: String,
    initial: String,
    validate: (String) -> String? = { null },
    /** An earlier change is still out and would make this one do nothing. */
    busy: Boolean = false,
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
    // Saved with its target: a restored draft is only used for the same one.
    var saved by rememberSaveable(stateSaver = NameDraftSaver) { mutableStateOf(target to initial) }
    val name = if (saved.first == target) saved.second else initial
    val cleaned = name.trim()
    val error = validate(cleaned)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { saved = target to it },
                singleLine = true,
                isError = error != null,
                supportingText = error?.let { { Text(it) } },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(cleaned) }, enabled = !busy && cleaned.isNotEmpty() && error == null) {
                Text("Rename")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

private val NameDraftSaver = listSaver<Pair<String, String>, String>(
    save = { listOf(it.first, it.second) },
    restore = { Pair(it[0], it[1]) },
)

/** A permanent removal, with the action named on its button rather than a bare Yes. */
@Composable
private fun ConfirmDeleteDialog(
    title: String,
    text: String,
    confirm: String,
    busy: Boolean = false,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(text) },
        confirmButton = {
            TextButton(onClick = onConfirm, enabled = !busy) { Text(confirm, color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The host names a project's folder after it, so the same rules apply. */
private fun projectNameError(name: String): String? = nameError("project", name)

/** A folder of projects is a directory on the host too. */
private fun folderNameError(name: String): String? = nameError("folder", name)

private fun nameError(kind: String, name: String): String? = when {
    name.startsWith('.') -> "A $kind name cannot begin with a period."
    '/' in name || '\\' in name -> "A $kind name cannot contain a path separator."
    else -> null
}

private class SessionViewModelOwner : ViewModelStoreOwner {
    override val viewModelStore: ViewModelStore = ViewModelStore()
}
