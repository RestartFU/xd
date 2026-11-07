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
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.model.MinimalProject
import com.restartfu.xd.model.MinimalSession
import com.restartfu.xd.model.minimalProjects
import com.restartfu.xd.model.minimalSessions
import com.restartfu.xd.net.Link
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarResult
import com.restartfu.xd.model.SettledSession
import com.restartfu.xd.model.activeSessions
import com.restartfu.xd.model.settledSessions
import com.restartfu.xd.protocol.Ops
import kotlinx.coroutines.launch

private enum class MobileDestination {
    PROJECTS,
    SESSIONS,
    TERMINAL,
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

    fun retain(chatIds: Set<String>, sessionIds: Set<String>) {
        // Panes can reference a chat connection, so release them first.
        workspaces.keys.filterNot { it in sessionIds }.forEach { workspaces.remove(it)?.viewModelStore?.clear() }
        chats.keys.filterNot { it in chatIds }.forEach { chats.remove(it)?.viewModelStore?.clear() }
    }

    private fun clearStores() {
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
    var renameChat by remember { mutableStateOf<ChatSummary?>(null) }
    var deleteChat by remember { mutableStateOf<ChatSummary?>(null) }
    val deleting by model.deletingChat.collectAsStateWithLifecycle()
    var renamingSession by remember { mutableStateOf<MinimalSession?>(null) }
    var deletingSession by remember { mutableStateOf<MinimalSession?>(null) }
    var renamingProject by remember { mutableStateOf<MinimalProject?>(null) }
    var deletingProject by remember { mutableStateOf<MinimalProject?>(null) }
    val settledNotice by model.settledNotice.collectAsStateWithLifecycle()
    val sessionActions = SessionActions(
        settlement = tree.settlement,
        settle = { model.settleSession(it.id, it.title) },
        unsettle = { model.unsettleSession(it.id) },
        setAutoSettle = { session, enabled -> model.setSessionAutoSettle(session.id, enabled) },
        rename = { renamingSession = it },
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
            XdNavigationRail(mobileDestination, showProjects, showSessions, showTerminal)
        }
        Column(Modifier.weight(1f)) {
            Box(Modifier.weight(1f)) {
                when {
                    mobileDestination == MobileDestination.TERMINAL -> MinimalGlobalTerminal(
                        model, settings, link, showProjects, showSessions,
                    )
                    mobileDestination == MobileDestination.SESSIONS && activeSession != null -> {
                        val tabStrip: @Composable () -> Unit = {
                            SessionTabStrip(
                                compact = !expanded,
                                chats = sessionChats,
                                hiddenChatIds = hiddenChatIds,
                                selectedChatId = activeChat?.id,
                                selectTab = { id ->
                                    hiddenChatIds = hiddenChatIds - id
                                    chatSelections = chatSelections + (activeSession.id to id)
                                },
                                closeTab = { id ->
                                    val index = openChats.indexOfFirst { it.id == id }
                                    val remaining = openChats.filterNot { it.id == id }
                                    if (id == activeChat?.id && remaining.isNotEmpty()) {
                                        chatSelections = chatSelections + (activeSession.id to remaining[index.coerceAtMost(remaining.lastIndex)].id)
                                    }
                                    hiddenChatIds = hiddenChatIds + id
                                },
                                addTab = { addChat(activeSession) },
                                renameChat = { renameChat = it },
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
                                    projectName = activeProject?.name.orEmpty(),
                                    workspaceId = activeSession.id,
                                    terminalChatId = activeSession.terminalChatId,
                                    workspaceOwner = stores.workspace(activeSession.id),
                                    sessionTabs = tabStrip,
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
                        onRenameProject = { renamingProject = it },
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
                XdNavigationBar(mobileDestination, showProjects, showSessions, showTerminal)
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
    renamingSession?.let { session ->
        NameDialog(
            title = "Rename session",
            initial = session.title,
            onDismiss = { renamingSession = null },
            onConfirm = { name ->
                renamingSession = null
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
    renamingProject?.let { project ->
        NameDialog(
            title = "Rename project",
            initial = project.name,
            validate = ::projectNameError,
            onDismiss = { renamingProject = null },
            onConfirm = { name ->
                renamingProject = null
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
        var title by rememberSaveable(chat.id) { mutableStateOf(chat.title) }
        AlertDialog(
            onDismissRequest = { renameChat = null },
            title = { Text("Rename chat") },
            text = {
                OutlinedTextField(title, onValueChange = { title = it }, label = { Text("Chat title") }, singleLine = true)
            },
            confirmButton = {
                TextButton(onClick = { model.renameChat(chat.id, title.trim()); renameChat = null }, enabled = title.isNotBlank()) { Text("Rename") }
            },
            dismissButton = { TextButton(onClick = { renameChat = null }) { Text("Cancel") } },
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
        NewSessionDialog(
            projectName = session.title,
            chatTab = true,
            onDismiss = { createChatIn = null },
            onCreate = { title, agent ->
                createChatIn = null
                activeProject?.let { project -> model.createDirectSession(project.id, title, agent, session.id) }
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
    val settled = if (actions.settlement) tree.settledSessions() else emptyList()
    val operationError by model.error.collectAsStateWithLifecycle()
    var selectedId by rememberSaveable { mutableStateOf<String?>(null) }
    var createProject by rememberSaveable { mutableStateOf(false) }
    var createSession by rememberSaveable { mutableStateOf(false) }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }

    val selected = projects.firstOrNull { it.id == selectedId } ?: projects.firstOrNull()
    val sessions = selected?.let { tree.activeSessions(it.id) }.orEmpty()
    val waiting by model.client.waitingChats.collectAsStateWithLifecycle()
    val unread by model.client.unreadChats.collectAsStateWithLifecycle()
    val signalOf = { session: MinimalSession ->
        when {
            session.chatIds.any { it in waiting } -> SessionSignal.WAITING
            session.working -> SessionSignal.WORKING
            session.chatIds.any { it in unread } -> SessionSignal.DONE
            else -> SessionSignal.IDLE
        }
    }

    LaunchedEffect(selected?.id) {
        if (selectedId != selected?.id) selectedId = selected?.id
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .navigationBarsPadding(),
    ) {
        ProductHeader(link = link, title = "Projects", onSettings = { settingsOpen = true })
        ContextBar(title = "${projects.size} project${if (projects.size == 1) "" else "s"}") {
            TextButton(onClick = { createProject = true }) {
                Icon(painterResource(R.drawable.ic_add), contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text("New project")
            }
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
                if (projects.isEmpty()) {
                    item("empty-projects") {
                        EmptyCard("Create a workspace, then start a Codex, Claude, or JCode session.")
                    }
                } else {
                    items(projects, key = MinimalProject::id) { project ->
                        if (project.id == selected?.id) {
                            OpenDrawer(
                                project = project,
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
                                        onDelete = { onDeleteProject(project) },
                                    )
                                },
                            )
                        } else {
                            DrawerRow(
                                project = project,
                                selected = false,
                                signals = tree.activeSessions(project.id).map(signalOf),
                                onClick = { selectedId = project.id },
                            )
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
                    onClick = { createSession = true },
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

    if (createProject) {
        NewProjectDialog(
            model = model,
            onDismiss = { createProject = false },
            onCreate = { name, repository ->
                createProject = false
                model.createWorkspace(name, repository)
            },
        )
    }
    if (createSession && selected != null) {
        NewSessionDialog(
            projectName = selected.name,
            onDismiss = { createSession = false },
            onCreate = { title, agent ->
                createSession = false
                model.createDirectSession(selected.id, title, agent)
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
) {
    NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
        NavItem("Projects", R.drawable.ic_projects, active = active == MobileDestination.PROJECTS, onClick = onProjects)
        NavItem("Sessions", R.drawable.ic_sessions, active = active == MobileDestination.SESSIONS, onClick = onSessions)
        NavItem("Terminal", R.drawable.ic_terminal, active = active == MobileDestination.TERMINAL, onClick = onTerminal)
    }
}

@Composable
private fun XdNavigationRail(
    active: MobileDestination,
    onProjects: () -> Unit,
    onSessions: () -> Unit,
    onTerminal: () -> Unit,
) {
    NavigationRail(containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Spacer(Modifier.height(16.dp))
        listOf(
            Triple(MobileDestination.PROJECTS, "Projects", R.drawable.ic_projects),
            Triple(MobileDestination.SESSIONS, "Sessions", R.drawable.ic_sessions),
            Triple(MobileDestination.TERMINAL, "Terminal", R.drawable.ic_terminal),
        ).forEach { (destination, label, icon) ->
            NavigationRailItem(
                selected = active == destination,
                onClick = when (destination) {
                    MobileDestination.PROJECTS -> onProjects
                    MobileDestination.SESSIONS -> onSessions
                    MobileDestination.TERMINAL -> onTerminal
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
    DONE("Done"),
}

@Composable
private fun SessionSignal.color(): Color {
    val signals = LocalXdSignals.current
    return when (this) {
        SessionSignal.IDLE -> signals.faint
        SessionSignal.WORKING -> signals.working
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
private fun SignalMark(signal: SessionSignal, modifier: Modifier = Modifier) {
    when (signal) {
        SessionSignal.IDLE -> Unit
        SessionSignal.WAITING -> StatusPill("Waiting", signal.color(), modifier)
        SessionSignal.WORKING -> PulseDot(signal.color(), modifier)
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
        Icon(
            painterResource(R.drawable.ic_folder),
            contentDescription = null,
            tint = if (selected) MaterialTheme.colorScheme.onSurfaceVariant else faint,
            modifier = Modifier.size(15.dp),
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

/** What can be done to a project as a whole, from the "more" button on its open row. */
@Composable
private fun ProjectMenu(
    project: MinimalProject,
    onShortcuts: () -> Unit,
    onRename: () -> Unit,
    onDelete: () -> Unit,
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
            DropdownMenuItem(
                text = { Text("Prompt shortcuts") },
                onClick = { open = false; onShortcuts() },
            )
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Delete project", color = MaterialTheme.colorScheme.error) },
                onClick = { open = false; onDelete() },
            )
        }
    }
}

/** The current project, open, with one row per session beneath it. */
@Composable
private fun OpenDrawer(
    project: MinimalProject,
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
        DrawerRow(project = project, selected = true, signals = emptyList(), onClick = {}, menu = menu)
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
        SignalMark(signal)
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
private fun NewSessionDialog(
    projectName: String,
    chatTab: Boolean = false,
    onDismiss: () -> Unit,
    onCreate: (String, DirectAgent) -> Unit,
) {
    var title by rememberSaveable { mutableStateOf("") }
    var agent by rememberSaveable { mutableStateOf(DirectAgent.CODEX) }
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
                            onClick = { agent = candidate },
                            label = { Text(candidate.label) },
                            leadingIcon = { BackendIcon(candidate.wire, size = 16.dp) },
                        )
                    }
                }
            }
        },
        confirmButton = {
            Button(onClick = { onCreate(title.trim(), agent) }) {
                Text(if (chatTab) "Create chat" else "Create session")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun NewProjectDialog(
    model: MainViewModel,
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
                enabled = name.isNotBlank(),
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
    var updatingHost by rememberSaveable { mutableStateOf(false) }
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
                if (tree.settlement) AutoSettleSettings(model)
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
}

/**
 * The host's rules for settling sessions by itself, with the desktop's
 * wording. They belong to the host, so they keep running while no client is
 * connected.
 */
@Composable
private fun AutoSettleSettings(model: MainViewModel) {
    val rules by model.autoSettle.collectAsStateWithLifecycle()
    val error by model.autoSettleError.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { model.loadAutoSettle() }
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
    initial: String,
    validate: (String) -> String? = { null },
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
    var name by rememberSaveable(initial) { mutableStateOf(initial) }
    val cleaned = name.trim()
    val error = validate(cleaned)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                isError = error != null,
                supportingText = error?.let { { Text(it) } },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(cleaned) }, enabled = cleaned.isNotEmpty() && error == null) {
                Text("Rename")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** A permanent removal, with the action named on its button rather than a bare Yes. */
@Composable
private fun ConfirmDeleteDialog(
    title: String,
    text: String,
    confirm: String,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(text) },
        confirmButton = {
            TextButton(onClick = onConfirm) { Text(confirm, color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** The host names a project's folder after it, so the same rules apply. */
private fun projectNameError(name: String): String? = when {
    name.startsWith('.') -> "A project name cannot begin with a period."
    '/' in name || '\\' in name -> "A project name cannot contain a path separator."
    else -> null
}

private class SessionViewModelOwner : ViewModelStoreOwner {
    override val viewModelStore: ViewModelStore = ViewModelStore()
}
