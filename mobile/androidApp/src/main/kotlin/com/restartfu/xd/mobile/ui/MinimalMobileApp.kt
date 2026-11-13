@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.restartfu.xd.mobile.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.material3.LocalTextStyle
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
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
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.material3.ButtonDefaults
import androidx.lifecycle.ViewModel
import com.restartfu.xd.model.ChatSummary
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.ui.unit.Dp
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
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
import com.restartfu.xd.mobile.StartingSession
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.model.MinimalProject
import com.restartfu.xd.model.MinimalSession
import com.restartfu.xd.model.backgroundTasksLabel
import com.restartfu.xd.model.minimalProjects
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import androidx.compose.ui.layout.ContentScale
import com.restartfu.xd.model.FolderDestination
import com.restartfu.xd.model.MinimalFolder
import com.restartfu.xd.model.projectsInside
import com.restartfu.xd.model.ProjectTreeRow
import com.restartfu.xd.model.projectChoices
import com.restartfu.xd.model.ancestors
import com.restartfu.xd.model.moveDestinations
import com.restartfu.xd.model.organizationLogin
import com.restartfu.xd.model.projectTree
import com.restartfu.xd.model.minimalSessions
import com.restartfu.xd.net.Link
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarResult
import com.restartfu.xd.model.SettledSession
import com.restartfu.xd.model.listedSessions
import com.restartfu.xd.model.settledSessions
import com.restartfu.xd.model.sessionsAbove
import com.restartfu.xd.model.startedBelow
import com.restartfu.xd.model.PauseAllAction
import com.restartfu.xd.model.pauseActionFor
import com.restartfu.xd.model.waitingOnUser
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

/**
 * The page drawn for a destination. Projects stands in for a session that
 * went away, so this is what the transition between pages follows.
 */
private enum class WorkspacePage { PROJECTS, SESSION, TERMINAL, USAGE }

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
    // Said once for the whole app: every page's connection line reads it.
    CompositionLocalProvider(
        LocalConnectionNotice provides rememberConnectionNotice(link),
        // The line's Try now: the next attempt at once, in place of the wait.
        LocalConnectionRetry provides model.client::retryConnection,
    ) {
        MinimalMobileWorkspace(model, settings)
    }
}

@Composable
private fun MinimalMobileWorkspace(
    model: MainViewModel,
    settings: MobileSettings,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val waitingChats by model.client.waitingChats.collectAsStateWithLifecycle()
    val unreadChats by model.client.unreadChats.collectAsStateWithLifecycle()
    // How each of the open session's chats stands, for its tab and menu row.
    val chatStatusOf = { chat: ChatSummary -> chatStatus(chat, chat.id in waitingChats, chat.id in unreadChats) }
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
    // These four are saved by id, so a rotation does not close a form or a question half answered.
    var createChatInId by rememberSaveable { mutableStateOf<String?>(null) }
    // Saved by id, so a chat rename survives rotating the phone.
    var renameChatId by rememberSaveable { mutableStateOf<String?>(null) }
    val renameChat = tree.chats.firstOrNull { it.id == renameChatId }
    var deleteChatId by rememberSaveable { mutableStateOf<String?>(null) }
    val deleteChat = tree.chats.firstOrNull { it.id == deleteChatId }
    val deleting by model.deletingChat.collectAsStateWithLifecycle()
    val renamingChats by model.renamingChat.collectAsStateWithLifecycle()
    // Saved by id, so a rename in progress survives rotating the phone.
    var renamingSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingSessionId by rememberSaveable { mutableStateOf<String?>(null) }
    var renamingProjectId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingProjectId by rememberSaveable { mutableStateOf<String?>(null) }
    val settledNotice by model.settledNotice.collectAsStateWithLifecycle()
    // Held here, not in the projects list, so Back from a chat returns to the same place in it.
    val projectsList = rememberLazyListState()
    val sessionActions = SessionActions(
        settlement = tree.settlement,
        settle = { model.settleSession(it.id, it.title) },
        unsettle = { model.unsettleSession(it.id) },
        setAutoSettle = { session, enabled -> model.setSessionAutoSettle(session.id, enabled) },
        rename = { renamingSessionId = it.id },
        delete = { deletingSessionId = it.id },
    )
    // Worked out once per tree, not on every recomposition of the app.
    val projects = remember(tree) { tree.minimalProjects() }
    val sessionsByProject = remember(tree, projects) {
        projects.associate { project -> project.id to tree.minimalSessions(project.id) }
    }
    val sessions = remember(sessionsByProject) { sessionsByProject.values.flatten() }
    val activeSession = sessions.firstOrNull { it.id == activeSessionId }
    val activeProject = projects.firstOrNull { project ->
        sessionsByProject[project.id].orEmpty().any { it.id == activeSessionId }
    }
    val sessionChats = activeSession?.chatIds.orEmpty().mapNotNull { id -> tree.chats.firstOrNull { it.id == id } }
    val openChats = sessionChats.filterNot { it.id in hiddenChatIds }
    val activeChat = openChats.firstOrNull { it.id == chatSelections[activeSessionId] } ?: openChats.firstOrNull()

    LaunchedEffect(tree.chats, tree.loading, tree.error) {
        if (!tree.loading && tree.error == null) {
            stores.retain(tree.chats.map { it.id }.toSet(), sessions.map { it.id }.toSet())
        }
    }
    val place = listOf(mobileDestination, activeSessionId, chatSelections, hiddenChatIds)
    SideEffect { model.place = place }
    LaunchedEffect(created, tree.chats) {
        created?.let { result ->
            // Gone somewhere else since it was asked for: it stays in its project.
            if (result.place != null && result.place != place) {
                model.consumeCreatedDirectSession(result)
                return@let
            }
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
    val showProjects = {
        model.stopOpeningStartingSessions()
        mobileDestination = MobileDestination.PROJECTS
    }
    val showTerminal = {
        model.stopOpeningStartingSessions()
        mobileDestination = MobileDestination.TERMINAL
    }
    val showUsage = {
        model.stopOpeningStartingSessions()
        mobileDestination = MobileDestination.USAGE
    }
    val addChat: (MinimalSession) -> Unit = { session ->
        if (session.terminalChatId.startsWith("session:")) {
            model.stopOpeningStartingSessions()
            createChatInId = session.id
        } else {
            scope.launch { snackbar.showSnackbar("Update the connected host to add chats inside a session.") }
        }
    }
    BackHandler(enabled = mobileDestination != MobileDestination.PROJECTS, onBack = showProjects)
    // Which layout the window gets, from its width and its height: see [WindowLayout].
    val layout = rememberWindowLayout()
    val imeVisible = WindowInsets.isImeVisible
    val chatOpen = mobileDestination == MobileDestination.SESSIONS && activeChat != null
    val showBottomNavigation = !layout.rail && !imeVisible && !chatOpen
    // The floating bar is drawn over the page, which keeps this much of its bottom clear.
    val barInset = if (showBottomNavigation) FloatingBarClearance else 0.dp
    // Projects also stands in for a session that went away, so this follows the page shown.
    val projectsShown = when (mobileDestination) {
        MobileDestination.TERMINAL, MobileDestination.USAGE -> false
        MobileDestination.SESSIONS -> activeSession == null
        MobileDestination.PROJECTS -> true
    }

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
    // Animations turned off cut straight from one page to the next.
    val still = reducedMotion()
    val page = when {
        mobileDestination == MobileDestination.TERMINAL -> WorkspacePage.TERMINAL
        mobileDestination == MobileDestination.USAGE -> WorkspacePage.USAGE
        mobileDestination == MobileDestination.SESSIONS && activeSession != null -> WorkspacePage.SESSION
        else -> WorkspacePage.PROJECTS
    }
    // A brief word to the reader goes on the app's one snackbar, not a system toast.
    val notify: (String) -> Unit = remember(scope, snackbar) { { text -> scope.launch { snackbar.showSnackbar(text) } } }
    CompositionLocalProvider(LocalNotice provides notify) {
    // Clear of a cutout or a navigation bar at either side, as a phone on its
    // side has: the backdrop still runs under them, nothing else does.
    Box(Modifier.fillMaxSize().safeSidesPadding()) {
    Row(
        Modifier
            .fillMaxSize()
            .background(GlassPage)
            // Settings covers it; neither TalkBack nor a search should find what is beneath.
            .then(if (settingsOpen) Modifier.clearAndSetSemantics {} else Modifier),
    ) {
        if (layout.rail) {
            XdNavigationRail(mobileDestination, showProjects, showTerminal, showUsage)
        }
        Column(Modifier.weight(1f)) {
            Box(Modifier.weight(1f)) {
                AnimatedContent(
                    targetState = page,
                    modifier = Modifier.fillMaxSize(),
                    // Into a session and back out along one axis; between the
                    // places the bar switches, a fade through.
                    transitionSpec = {
                        pageMotion(
                            still,
                            when {
                                targetState == WorkspacePage.SESSION -> sharedAxisX(forward = true)
                                initialState == WorkspacePage.SESSION -> sharedAxisX(forward = false)
                                else -> fadeThrough()
                            },
                        )
                    },
                    label = "page",
                ) { shown ->
                when (shown) {
                    WorkspacePage.TERMINAL -> MinimalGlobalTerminal(
                        model, barInset, onSettings = { settingsOpen = true },
                    )
                    WorkspacePage.USAGE -> Column(
                        Modifier.fillMaxSize().background(GlassPage),
                    ) {
                        ProductHeader(title = "Usage", onSettings = { settingsOpen = true })
                        // Kept with the other per-host stores, so leaving Usage keeps what it read.
                        val usage: UsageViewModel = viewModel(
                            key = "usage",
                            viewModelStoreOwner = stores.usage(),
                            factory = UsageViewModel.Factory(model.client, settings),
                        )
                        UsageContent(usage, bottomInset = barInset)
                    }
                    // Gone while its page is still leaving: nothing is drawn for those frames.
                    WorkspacePage.SESSION -> if (activeSession != null) {
                        val selectTab = { id: String ->
                            // A tab picked: a chat still starting does not take its place.
                            model.stopOpeningStartingSessions()
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
                        val tabStrip: @Composable (AgentTabs, (() -> Unit)?) -> Unit = { agents, showPullRequests ->
                            SessionTabStrip(
                                compact = !layout.tabletChat,
                                chats = sessionChats,
                                hiddenChatIds = hiddenChatIds,
                                selectedChatId = activeChat?.id,
                                selectTab = selectTab,
                                closeTab = closeTab,
                                addTab = { addChat(activeSession) },
                                renameChat = { renameChatId = it.id },
                                deleteChat = { deleteChatId = it.id },
                                session = activeSession,
                                actions = sessionActions,
                                agents = agents,
                                showPullRequests = showPullRequests,
                                statusOf = chatStatusOf,
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
                                    model = chat, goBack = showProjects,
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
                                    // tabs into it, upright or on its side; a
                                    // window with a tablet's height has room for the tabs.
                                    sideBySide = layout.sideBySide,
                                    sessionChats = if (layout.tabletChat) null else SessionChats(
                                        chats = sessionChats,
                                        selectedId = activeChat.id,
                                        select = selectTab,
                                        add = { addChat(activeSession) },
                                        close = closeTab,
                                        rename = { renameChatId = it.id },
                                        delete = { deleteChatId = it.id },
                                        settleSession = { sessionActions.settle(activeSession) }
                                            .takeIf { sessionActions.settlement && !activeSession.settled },
                                        renameSession = { sessionActions.rename(activeSession) },
                                        statusOf = chatStatusOf,
                                    ),
                                    linkedPullRequests = activeChat.pullRequests.takeIf { tree.linkedPullRequests },
                                    agentChats = tree.chats.filter { it.parentId == activeChat.id }
                                        .map { it.waitingOnUser(it.id in waitingChats) }
                                        .associateBy { it.id },
                                    startedBy = activeChat.parentId?.let { parent -> tree.chats.firstOrNull { it.id == parent } },
                                    openChat = { id ->
                                        tree.chats.firstOrNull { it.id == id }?.let { chat ->
                                            // Unfolded on the way, so its row shows on the Projects page.
                                            settings.setSessionsFolded(tree.sessionsAbove(chat.sessionId), false)
                                            activeSessionId = chat.sessionId
                                            chatSelections = chatSelections + (chat.sessionId to chat.id)
                                            hiddenChatIds = hiddenChatIds - chat.id
                                            mobileDestination = MobileDestination.SESSIONS
                                        }
                                    },
                                )
                            }
                        } else {
                            Column(Modifier.fillMaxSize()) {
                                ProductHeader(activeSession.title, onSettings = { settingsOpen = true })
                                Column(Modifier.background(glassPane())) { tabStrip(AgentTabs(), null) }
                                Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
                                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                        Text("Your chats are saved", style = MaterialTheme.typography.titleLarge)
                                        Text("Reopen a chat from the tab menu, or start a new one.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                                        Button(onClick = { addChat(activeSession) }) { Text("New chat") }
                                    }
                                }
                            }
                        }
                    } else Unit
                    WorkspacePage.PROJECTS -> MinimalProjectsHome(
                        model, settings, projects, tree.loading, tree.error,
                        onSettings = { settingsOpen = true },
                        listState = projectsList,
                        newSessionAboveBar = showBottomNavigation,
                        actions = sessionActions,
                        onRenameProject = { renamingProjectId = it.id },
                        onDeleteProject = { deletingProjectId = it.id },
                        open = { _, session ->
                            model.stopOpeningStartingSessions()
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
                    XdNavigationBar(
                        mobileDestination, showProjects, showTerminal, showUsage,
                        Modifier.align(Alignment.BottomCenter),
                    )
                }
            }
        }
    }
    SnackbarHost(
        snackbar,
        Modifier
            .align(Alignment.BottomCenter)
            .navigationBarsPadding()
            .padding(horizontal = 16.dp)
            // Bottom only: a short split-screen window still leaves the snackbar its height.
            .padding(
                bottom = when {
                    // Above Projects' New session button, as Material places snackbars over a FAB.
                    showBottomNavigation && projectsShown -> NewSessionBottom + FloatingBarHeight + 8.dp
                    showBottomNavigation -> FloatingBarClearance
                    else -> 24.dp
                },
            ),
    )
    // Over everything, rail and bars included: Settings is its own page,
    // stepped into from whichever page its button was on.
    AnimatedVisibility(
        visible = settingsOpen,
        enter = if (still) EnterTransition.None else sharedAxisEnter(forward = true),
        exit = if (still) ExitTransition.None else sharedAxisExit(forward = false),
    ) {
        SettingsPage(
            model = model,
            settings = settings,
            onClose = { settingsOpen = false },
            onForget = {
                settingsOpen = false
                model.forget()
            },
        )
    }
    }
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
    sessions.firstOrNull { it.id == deletingSessionId }?.let { session ->
        ConfirmDeleteDialog(
            title = "Delete session?",
            busy = !session.grouped && deleting,
            text = "This permanently removes ${session.title} and all its chats from xd. Project files are kept.",
            confirm = "Delete session",
            onDismiss = { deletingSessionId = null },
            onConfirm = {
                deletingSessionId = null
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
    projects.firstOrNull { it.id == deletingProjectId }?.let { project ->
        ConfirmDeleteDialog(
            title = "Delete project?",
            text = "This removes ${project.name} and all its sessions from xd. Its files will be moved to the workspace trash.",
            confirm = "Delete project",
            onDismiss = { deletingProjectId = null },
            onConfirm = {
                deletingProjectId = null
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
            onDismissRequest = { if (!deleting) deleteChatId = null },
            title = { Text("Delete chat?") },
            text = {
                Text(
                    "This permanently removes ${chat.title.ifBlank { "New chat" }} and its conversation from xd. " +
                        "Project files are kept.",
                )
            },
            confirmButton = {
                TextButton(onClick = { model.deleteChat(chat.id); deleteChatId = null }, enabled = !deleting) {
                    Text("Delete chat", color = MaterialTheme.colorScheme.error)
                }
            },
            dismissButton = { TextButton(onClick = { deleteChatId = null }, enabled = !deleting) { Text("Cancel") } },
        )
    }
    sessions.firstOrNull { it.id == createChatInId }?.let { session ->
        LaunchedEffect(session.id) { model.loadCreationCatalog() }
        NewSessionDialog(
            projectName = session.title,
            draftKey = "${activeProject?.id}/${session.id}",
            backends = model.hostCatalog.collectAsStateWithLifecycle().value,
            catalogError = model.hostCatalogError.collectAsStateWithLifecycle().value,
            retryCatalog = model::loadCreationCatalog,
            accounts = rememberChatAccounts(model),
            chatTab = true,
            onDismiss = { createChatInId = null },
            onCreate = { request ->
                createChatInId = null
                activeProject?.let { project ->
                    model.createDirectSession(
                        project.id, request.title, request.agent, session.id,
                        effort = request.effort, access = request.access, plan = request.plan,
                        account = request.account,
                    )
                }
            },
        )
    }
}

@Composable
private fun MinimalProjectsHome(
    model: MainViewModel,
    settings: MobileSettings,
    projects: List<MinimalProject>,
    loading: Boolean,
    treeError: String?,
    onSettings: () -> Unit,
    listState: LazyListState,
    newSessionAboveBar: Boolean,
    actions: SessionActions,
    onRenameProject: (MinimalProject) -> Unit,
    onDeleteProject: (MinimalProject) -> Unit,
    open: (MinimalProject, MinimalSession) -> Unit,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val starting by model.startingSessions.collectAsStateWithLifecycle()
    var shelfOpen by rememberSaveable { mutableStateOf(false) }
    // Saved by project id, so an open editor survives recreating the activity.
    var contextForId by rememberSaveable { mutableStateOf<String?>(null) }
    var secretsForId by rememberSaveable { mutableStateOf<String?>(null) }
    // Folders of projects have context and secrets too, which reach every project inside.
    val contextFor = tree.folders.firstOrNull { it.id == contextForId }
    val secretsFor = tree.folders.firstOrNull { it.id == secretsForId }
    val settled = remember(tree, actions.settlement) {
        if (actions.settlement) tree.settledSessions() else emptyList()
    }
    val operationError by model.error.collectAsStateWithLifecycle()
    // What the New menu or a folder's menu is making ("project" or "folder"), and in which folder.
    var creating by rememberSaveable { mutableStateOf<String?>(null) }
    var createParentId by rememberSaveable { mutableStateOf<String?>(null) }
    var renamingFolderId by rememberSaveable { mutableStateOf<String?>(null) }
    var deletingFolderId by rememberSaveable { mutableStateOf<String?>(null) }
    var movingId by rememberSaveable { mutableStateOf<String?>(null) }
    val collapsed by settings.collapsedFolders.collectAsStateWithLifecycle()
    val expandedSessions by settings.expandedSessions.collectAsStateWithLifecycle()
    // An earlier project or folder is still being made; another waits rather than being dropped.
    val creatingWorkspace by model.creatingWorkspace.collectAsStateWithLifecycle()
    val ownerIcons by model.client.ownerIcons.collectAsStateWithLifecycle()
    // Built again only when the tree or what is folded changes, not each time the page is drawn.
    val rows = remember(tree, collapsed) { tree.projectTree(collapsed) }
    // Every row, as if no folder were closed: built once per tree, since building it reads every chat.
    val openTree = remember(tree) { tree.projectTree() }
    val folders = remember(openTree) { openTree.mapNotNull { (it as? ProjectTreeRow.FolderRow)?.folder } }
    // Opens the folders [id] sits in, and [id] itself, so what lands there is in view.
    val reveal = { id: String? ->
        if (id != null) (tree.ancestors(id) + id).forEach { settings.setFolderOpen(it, true) }
    }
    val newProjectIn = { parentId: String? -> createParentId = parentId; creating = "project" }
    val newFolderIn = { parentId: String? -> createParentId = parentId; creating = "folder" }
    // Asked for every row drawn, and each answer walks the folders: answered once per item and tree.
    val movable = remember(tree) { HashMap<String, Boolean>() }
    val canMove = { id: String, parentId: String? ->
        parentId != null || movable.getOrPut(id) { tree.moveDestinations(id).isNotEmpty() }
    }
    // The project New session was opened for; it closes if that project goes away.
    var createSessionIn by rememberSaveable { mutableStateOf<String?>(null) }
    // New session asks which project first, unless there is only one.
    var pickingProject by rememberSaveable { mutableStateOf(false) }
    // Opened on the list, so Back from the form returns to it.
    var startedFromList by rememberSaveable { mutableStateOf(false) }
    // Opening another form moves on from a session still starting: it is not
    // opened when made.
    val newSession = {
        model.stopOpeningStartingSessions()
        val only = projects.singleOrNull()
        if (only != null) {
            createSessionIn = only.id
            startedFromList = false
        } else {
            pickingProject = true
            startedFromList = true
        }
    }
    val newSessionIn = { projectId: String ->
        model.stopOpeningStartingSessions()
        createSessionIn = projectId
        pickingProject = false
        startedFromList = false
    }
    val waiting by model.client.waitingChats.collectAsStateWithLifecycle()
    val unread by model.client.unreadChats.collectAsStateWithLifecycle()
    val signalOf = { session: MinimalSession ->
        when {
            // A chat stopped at its quota waits on the user's answer too.
            session.quotaReached || session.chatIds.any { it in waiting } -> SessionSignal.WAITING
            session.working -> SessionSignal.WORKING
            session.backgroundTasks > 0 -> SessionSignal.BACKGROUND
            session.paused -> SessionSignal.PAUSED
            session.ownChatIds.any { it in unread } -> SessionSignal.DONE
            else -> SessionSignal.IDLE
        }
    }
    // An agent's session lists while it has something going on: its lead got
    // the report of a finished one, and its card in the lead's chat opens it.
    // Worked out once per project, tree and set of waiting chats: every row
    // drawn asks for it, and each answer reads the project's every session.
    val listedByProject = remember(tree, waiting) { HashMap<String, List<MinimalSession>>() }
    val listed = { projectId: String ->
        listedByProject.getOrPut(projectId) {
            tree.listedSessions(projectId) { session -> session.chatIds.any { it in waiting } }
        }
    }
    // What a session passes up to a folded or closed row above it: an agent's
    // session passes nothing but a wait on the user, since its lead already
    // says it waits on its agents.
    val outwardOf = { session: MinimalSession ->
        signalOf(session).takeIf { !session.startedByAgent || it == SessionSignal.WAITING }
            ?: SessionSignal.IDLE
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(GlassPage)
            .navigationBarsPadding(),
    ) {
        val pausingAll by model.pausingAll.collectAsStateWithLifecycle()
        // Each project's own pause button, beside its +; hidden with nothing to pause.
        val pauseActions = remember(tree) { HashMap<String, PauseAllAction?>() }
        val pauseFor: (MinimalProject) -> (@Composable () -> Unit)? = { project ->
            // Looked up by key, since "nothing to pause" is an answer worth keeping too.
            val action = if (project.id in pauseActions) pauseActions[project.id]
            else tree.pauseActionFor(project.id).also { pauseActions[project.id] = it }
            action?.let { action ->
                {
                    PauseProjectButton(
                        projectName = project.name,
                        action = action,
                        enabled = !pausingAll,
                        onClick = {
                            if (action == PauseAllAction.RESUME) model.resumeProject(project.id)
                            else model.pauseProject(project.id)
                        },
                    )
                }
            }
        }
        ProductHeader(title = "Projects", onSettings = onSettings)
        when {
            loading && projects.isEmpty() -> Box(
                Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }

            else -> BoxWithConstraints(Modifier.fillMaxSize()) {
            // As wide as the page, so it scrolls from its margins; its rows stop at the reading measure.
            val side = readingInset(maxWidth, 12.dp)
            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                state = listState,
                contentPadding = PaddingValues(side, 14.dp, side, if (newSessionAboveBar) NewSessionBottom + FloatingBarHeight + 12.dp else 96.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                (treeError ?: operationError)?.let { error ->
                    item("error") {
                        Row(
                            Modifier.fillMaxWidth().padding(start = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                error,
                                // Told as it appears, since the list may look as it did before.
                                modifier = Modifier
                                    .weight(1f)
                                    .padding(vertical = 8.dp)
                                    .semantics { liveRegion = LiveRegionMode.Polite },
                                color = MaterialTheme.colorScheme.error,
                            )
                            // A tree that did not load is read again from here.
                            if (treeError != null) {
                                TextButton(onClick = model.client::refreshTree, enabled = !loading) { Text("Try again") }
                            }
                        }
                    }
                }
                if (rows.isEmpty()) {
                    // With nothing read there is nothing to suggest making yet.
                    if (treeError == null) {
                        item("empty-projects") {
                            EmptyCard("Create a project, then start a Codex or Claude session in it.")
                        }
                    }
                } else {
                    items(rows, key = ProjectTreeRow::id) { row ->
                        val indent = Modifier.padding(start = (row.depth * FOLDER_INDENT).dp)
                        when (row) {
                            is ProjectTreeRow.FolderRow -> FolderRow(
                                folder = row.folder,
                                open = row.open,
                                icon = ownerIcons[row.folder.owner],
                                // A closed folder hides its projects, so it carries their signals.
                                signals = if (row.open) emptyList() else openTree.projectsInside(row.folder.id)
                                    .flatMap { listed(it).map(outwardOf) },
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
                                // Projects open and close on their own, like folders,
                                // so several can be open at once.
                                val projectOpen = project.id !in collapsed
                                val toggle = { settings.setFolderOpen(project.id, !projectOpen) }
                                val menu: @Composable () -> Unit = {
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
                                }
                                if (projectOpen) {
                                    OpenDrawer(
                                        project = project,
                                        icon = ownerIcons[project.owner],
                                        sessions = listed(project.id),
                                        starting = starting.filter { it.projectId == project.id && it.sessionId == null },
                                        signalOf = signalOf,
                                        outwardOf = outwardOf,
                                        open = { session -> open(project, session) },
                                        actions = actions,
                                        expanded = expandedSessions,
                                        fold = { session, folded -> settings.setSessionsFolded(listOf(session.id), folded) },
                                        toggle = toggle,
                                        onNewSession = { newSessionIn(project.id) },
                                        pause = pauseFor(project),
                                        menu = menu,
                                    )
                                } else {
                                    DrawerRow(
                                        project = project,
                                        open = false,
                                        icon = ownerIcons[project.owner],
                                        signals = listed(project.id).map(outwardOf),
                                        onClick = toggle,
                                        pause = pauseFor(project),
                                        menu = menu,
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
            // The page's one New button: a session, a project, or a folder.
            // On phones it sits just above the floating bar's right end, clear of it at any width.
            // The tooltip's own layout drops alignment, so a plain box places it.
            Box(
                Modifier
                    .align(Alignment.BottomEnd)
                    .padding(end = 16.dp, bottom = if (newSessionAboveBar) NewSessionBottom else 16.dp),
            ) {
                NewMenu(
                    canStartSession = projects.isNotEmpty(),
                    canCreateFolders = tree.canCreateFolders,
                    onSession = newSession,
                    onProject = { newProjectIn(null) },
                    onFolder = { newFolderIn(null) },
                ) { open ->
                    if (newSessionAboveBar) {
                        IconTooltip("New") {
                            FloatingActionButton(
                                onClick = open,
                                modifier = Modifier.size(FloatingBarHeight),
                                shape = CircleShape,
                                containerColor = MaterialTheme.colorScheme.primary,
                                contentColor = MaterialTheme.colorScheme.onPrimary,
                            ) {
                                Icon(painterResource(R.drawable.ic_add), contentDescription = "New")
                            }
                        }
                    } else {
                        ExtendedFloatingActionButton(
                            onClick = open,
                            icon = { Icon(painterResource(R.drawable.ic_add), contentDescription = null) },
                            text = { Text("New") },
                            containerColor = MaterialTheme.colorScheme.primary,
                            contentColor = MaterialTheme.colorScheme.onPrimary,
                        )
                    }
                }
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
    val lastSessionProject by settings.lastSessionProject.collectAsStateWithLifecycle()
    val creatingIn = projects.firstOrNull { it.id == createSessionIn }
    LaunchedEffect(createSessionIn, creatingIn == null, loading) {
        if (createSessionIn != null && creatingIn == null && !loading) createSessionIn = null
    }
    if (pickingProject || creatingIn != null) {
        LaunchedEffect(Unit) { model.loadCreationCatalog() }
        NewSessionFlow(
            choices = tree.projectChoices(lastSessionProject),
            lastUsed = lastSessionProject,
            icons = ownerIcons,
            projectId = creatingIn?.id.takeUnless { pickingProject },
            returnsToList = startedFromList,
            onChoose = { id ->
                createSessionIn = id
                pickingProject = id == null
                // Change goes back to the list, and Back then returns to it too.
                if (id == null) startedFromList = true
            },
            backends = model.hostCatalog.collectAsStateWithLifecycle().value,
            catalogError = model.hostCatalogError.collectAsStateWithLifecycle().value,
            retryCatalog = model::loadCreationCatalog,
            accounts = rememberChatAccounts(model),
            places = { id -> NewSessionPlaces.load(model.client, id) },
            onDismiss = {
                createSessionIn = null
                pickingProject = false
            },
            onCreate = { projectId, request ->
                createSessionIn = null
                pickingProject = false
                settings.setLastSessionProject(projectId)
                // Coming back to Projects, the new session's project is in view.
                reveal(projectId)
                model.createDirectSession(
                    projectId, request.title, request.agent,
                    worktree = request.worktree, startFromRemote = request.startFromRemote,
                    effort = request.effort, access = request.access, plan = request.plan,
                    account = request.account,
                )
            },
        )
    }
}

@Composable
private fun MinimalGlobalTerminal(
    model: MainViewModel,
    bottomInset: Dp,
    onSettings: () -> Unit,
) {
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
    val state by terminal.state.collectAsStateWithLifecycle()

    Column(
        Modifier
            .fillMaxSize()
            .background(GlassPage)
            .navigationBarsPadding(),
    ) {
        // The header names the page once and holds the shell's one action,
        // in place of a second row that only repeated "Terminal".
        ProductHeader(title = "Terminal", onSettings = onSettings) {
            // A shell that failed to open says Try again under its error instead.
            if (state.closed) {
                TextButton(onClick = terminal::open) { Text("Reopen") }
            } else if (state.error == null) {
                TextButton(
                    onClick = terminal::kill,
                    enabled = state.id != null,
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) { Text("Stop") }
            }
        }
        Box(
            modifier = Modifier
                .fillMaxSize()
                .padding(bottom = bottomInset)
                .padding(10.dp)
                .clip(MaterialTheme.shapes.medium)
                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, MaterialTheme.shapes.medium),
        ) {
            TerminalPaneContent(terminal, showSessionBar = false)
        }
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
    /** The open chat's agent tabs, after its chat tabs. */
    agents: AgentTabs = AgentTabs(),
    /** Opens the open chat's Pull requests list; null where the host cannot link them. */
    showPullRequests: (() -> Unit)? = null,
    /** How a chat stands, marked on its tab as on the desktop's; null for a quiet one. */
    statusOf: (ChatSummary) -> StatusMark? = { null },
) {
    var openMenu by remember { mutableStateOf(false) }
    Row(
        // Clear: it sits on its header's one pane of glass.
        modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            modifier = Modifier.weight(1f).horizontalScroll(rememberScrollState()).padding(vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            chats.filterNot { it.id in hiddenChatIds }.forEach { chat ->
                val selected = chat.id == selectedChatId && agents.viewing == null
                val reveal = remember(chat.id) { BringIntoViewRequester() }
                LaunchedEffect(chat.id, selected) {
                    if (selected) {
                        androidx.compose.runtime.withFrameNanos { }
                        reveal.bringIntoView()
                    }
                }
                Surface(
                    modifier = Modifier.bringIntoViewRequester(reveal),
                    shape = MaterialTheme.shapes.medium,
                    color = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                    border = if (selected) androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant) else null,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Row(
                            modifier = Modifier.heightIn(min = 48.dp).clickable(role = Role.Tab) { agents.showChat(); selectTab(chat.id) }.padding(start = 12.dp, end = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            BackendIcon(chat.backend, size = 16.dp)
                            Text(chat.title.ifBlank { "New chat" }, modifier = Modifier.widthIn(max = 140.dp),
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.labelLarge,
                                color = if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant)
                            statusOf(chat)?.let { StatusMarkIcon(it) }
                        }
                        if (!compact) {
                            IconButton(onClick = { closeTab(chat.id) }, modifier = Modifier.size(48.dp)) {
                                Icon(painterResource(R.drawable.ic_close), "Close ${chat.title}", modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                }
            }
            agents.tabs.forEach { agent ->
                key(agent.key) { AgentTabChip(agent, selected = agent.key == agents.viewing, agents) }
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
                    val status = statusOf(chat)
                    DropdownMenuItem(
                        text = { Text(chat.title.ifBlank { "New chat" }, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        leadingIcon = { BackendIcon(chat.backend, size = 18.dp) },
                        trailingIcon = status?.let { { StatusMarkIcon(it) } },
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
                    showPullRequests?.let { show ->
                        DropdownMenuItem(
                            text = { Text("Pull requests") },
                            onClick = { openMenu = false; show() },
                        )
                    }
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

/** One of the open chat's agents in the tab row: its provider, task, a dot while it runs, and close. */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun AgentTabChip(agent: AgentTab, selected: Boolean, agents: AgentTabs) {
    val reveal = remember { BringIntoViewRequester() }
    LaunchedEffect(selected) {
        if (selected) {
            androidx.compose.runtime.withFrameNanos { }
            reveal.bringIntoView()
        }
    }
    Surface(
        modifier = Modifier.bringIntoViewRequester(reveal),
        shape = MaterialTheme.shapes.medium,
        color = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
        border = if (selected) androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant) else null,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Row(
                modifier = Modifier.heightIn(min = 48.dp).clickable(role = Role.Tab) { agents.open(agent.key) }.padding(start = 12.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                BackendIcon(agent.provider.lowercase(), size = 16.dp)
                Text(
                    agent.title, modifier = Modifier.widthIn(max = 140.dp),
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.labelLarge,
                    color = if (selected) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (agent.running) StatusMarkIcon(StatusMark.WORKING)
            }
            IconButton(onClick = { agents.close(agent.key) }, modifier = Modifier.size(48.dp)) {
                Icon(painterResource(R.drawable.ic_close), "Close ${agent.title}", modifier = Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

/**
 * A top-level page's header: the mark, the page's name, its own action if it
 * has one, a dot while the host is connected, and Settings. While the host is
 * not connected the dot is gone and the line below says so in words.
 */
@Composable
private fun ProductHeader(
    title: String,
    onSettings: () -> Unit,
    action: (@Composable () -> Unit)? = null,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(glassPane())
            .safeTopPadding()
            // At least, not exactly: enlarged text makes the title taller.
            .heightIn(min = 56.dp)
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
            modifier = Modifier.weight(1f).semantics { heading() },
            style = MaterialTheme.typography.titleMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        action?.invoke()
        Box(
            modifier = Modifier.size(48.dp),
            contentAlignment = Alignment.Center,
        ) {
            if (LocalConnectionNotice.current.value == null) {
                Box(
                    Modifier
                        .size(9.dp)
                        .clip(CircleShape)
                        .background(LocalXdSignals.current.working)
                        .semantics { contentDescription = "Connected" },
                )
            }
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
    ConnectionLine()
}

/**
 * Pauses a project's working chats for stepping away from work, then turns
 * into the button that resumes them.
 */
@Composable
private fun PauseProjectButton(
    projectName: String,
    action: PauseAllAction,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    val resume = action == PauseAllAction.RESUME
    val label = if (resume) "Resume chats in $projectName" else "Pause chats in $projectName"
    IconTooltip(label) {
        IconButton(onClick = onClick, enabled = enabled, modifier = Modifier.size(48.dp)) {
            Icon(
                painter = painterResource(if (resume) R.drawable.ic_play else R.drawable.ic_pause),
                contentDescription = label,
                tint = if (resume) MaterialTheme.colorScheme.primary
                else MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(16.dp),
            )
        }
    }
}

/** Height of the floating bar, and the size of the round New session button above it. */
private val FloatingBarHeight = 60.dp

/** What the floating bar covers at the foot of a page: its margin, its height, and a gap above it. */
private val FloatingBarClearance = 16.dp + FloatingBarHeight + 12.dp

/** Where the New session button's bottom sits: 8dp above the floating bar. */
private val NewSessionBottom = 16.dp + FloatingBarHeight + 8.dp

private val FloatingTabWidth = 56.dp
private val FloatingTabGap = 4.dp
private val FloatingBarPadding = 6.dp

/**
 * Icon tabs between the top-level places, floating over the page on phones.
 * Sessions open from Projects. A long press names each tab.
 */
@Composable
private fun XdNavigationBar(
    active: MobileDestination,
    onProjects: () -> Unit,
    onTerminal: () -> Unit,
    onUsage: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        modifier = modifier.navigationBarsPadding().padding(bottom = 16.dp).height(FloatingBarHeight),
        shape = CircleShape,
        color = glassOverlay(),
        border = BorderStroke(1.dp, glassEdge()),
        shadowElevation = 6.dp,
    ) {
        Row(
            Modifier.selectableGroup().padding(FloatingBarPadding),
            horizontalArrangement = Arrangement.spacedBy(FloatingTabGap),
        ) {
            listOf(
                Triple(MobileDestination.PROJECTS, "Projects", R.drawable.ic_projects) to onProjects,
                Triple(MobileDestination.TERMINAL, "Terminal", R.drawable.ic_terminal) to onTerminal,
                Triple(MobileDestination.USAGE, "Usage", R.drawable.ic_usage) to onUsage,
            ).forEach { (item, onClick) ->
                val (destination, label, icon) = item
                val selected = active == destination
                IconTooltip(label) {
                    Box(
                        Modifier
                            .size(width = FloatingTabWidth, height = 48.dp)
                            .clip(CircleShape)
                            .background(if (selected) colors.surfaceContainerHighest else Color.Transparent)
                            .selectable(selected = selected, role = Role.Tab, onClick = onClick)
                            .semantics { contentDescription = label },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            painterResource(icon),
                            contentDescription = null,
                            tint = if (selected) colors.onSurface else colors.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/** Names an unlabelled control when it is long-pressed. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun IconTooltip(label: String, content: @Composable () -> Unit) {
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = { PlainTooltip { Text(label) } },
        state = rememberTooltipState(),
        content = content,
    )
}

@Composable
private fun XdNavigationRail(
    active: MobileDestination,
    onProjects: () -> Unit,
    onTerminal: () -> Unit,
    onUsage: () -> Unit,
) {
    NavigationRail(containerColor = glassPane()) {
        Spacer(Modifier.height(16.dp))
        listOf(
            Triple(MobileDestination.PROJECTS, "Projects", R.drawable.ic_projects) to onProjects,
            Triple(MobileDestination.TERMINAL, "Terminal", R.drawable.ic_terminal) to onTerminal,
            Triple(MobileDestination.USAGE, "Usage", R.drawable.ic_usage) to onUsage,
        ).forEach { (item, onClick) ->
            val (destination, label, icon) = item
            NavigationRailItem(
                selected = active == destination,
                onClick = onClick,
                icon = { Icon(painterResource(icon), contentDescription = null) },
                label = { Text(label) },
            )
            Spacer(Modifier.height(8.dp))
        }
    }
}

/** What a session's signal tab says about its agent, as on the desktop. */
internal enum class SessionSignal(val label: String, val mark: StatusMark? = null) {
    IDLE("Idle"),
    WORKING("Working", StatusMark.WORKING),
    WAITING("Waiting on you", StatusMark.WAITING),
    /** No turn running, but background agents or shells are. */
    BACKGROUND("Waiting on background tasks"),
    /** Stopped by pausing every chat; resuming them continues it. */
    PAUSED("Paused", StatusMark.PAUSED),
    DONE("Done", StatusMark.DONE),
}

@Composable
private fun SessionSignal.color(): Color =
    mark?.color() ?: if (this == SessionSignal.BACKGROUND) MaterialTheme.colorScheme.onSurfaceVariant
    else LocalXdSignals.current.faint

/**
 * What a closed row says of the sessions it hides, for a screen reader: how
 * many stand each way, the ones that need the reader first. Quiet and paused
 * sessions are left out, as their marks are.
 */
internal fun signalSummary(signals: List<SessionSignal>): String =
    listOf(SessionSignal.WAITING, SessionSignal.WORKING, SessionSignal.BACKGROUND, SessionSignal.DONE)
        .mapNotNull { signal ->
            val count = signals.count { it == signal }
            if (count == 0) null else "$count ${signal.label.lowercase()}"
        }
        .joinToString(", ")

/**
 * The status at the end of a session row: nothing for a quiet session, the
 * shared mark for work and unread turns, and the mark with its words for the
 * two states that ask something of the reader. [wordless] leaves the words
 * out where the row says them elsewhere.
 */
@Composable
private fun SignalMark(
    signal: SessionSignal,
    modifier: Modifier = Modifier,
    backgroundTasks: Int = 0,
    wordless: Boolean = false,
) {
    when (signal) {
        SessionSignal.IDLE -> Unit
        SessionSignal.WAITING, SessionSignal.PAUSED -> {
            val mark = checkNotNull(signal.mark)
            if (wordless) StatusMarkIcon(mark, modifier, labelled = false)
            else StatusPill(signal.label, signal.color(), modifier, mark = mark)
        }
        SessionSignal.WORKING, SessionSignal.DONE -> StatusMarkIcon(checkNotNull(signal.mark), modifier)
        // The count beside a still ring; the words are for screen readers.
        SessionSignal.BACKGROUND -> Row(
            modifier.clearAndSetSemantics { contentDescription = backgroundTasksLabel(backgroundTasks) },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text("$backgroundTasks", color = signal.color(), style = MaterialTheme.typography.labelSmall)
            BackgroundRing(signal.color())
        }
    }
}

/** A collapsed project: one line, with a dot for each session that needs a look. */
@Composable
private fun DrawerRow(
    project: MinimalProject,
    open: Boolean,
    signals: List<SessionSignal>,
    /** Opens or closes the project. */
    onClick: () -> Unit,
    /** The avatar of the account behind the project, in place of the folder icon. */
    icon: ByteArray? = null,
    /** Starts a session here; offered on an open project. */
    onNewSession: (() -> Unit)? = null,
    /** Pauses or resumes the project's chats, just before its +. */
    pause: (@Composable () -> Unit)? = null,
    menu: (@Composable () -> Unit)? = null,
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(MaterialTheme.shapes.small)
            .clickable(
                role = Role.Button,
                onClickLabel = if (open) "Close project" else "Open project",
                onClick = onClick,
            )
            .semantics { stateDescription = if (open) "Expanded" else "Collapsed" }
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
        OwnerAvatar(
            icon = icon,
            fallback = R.drawable.ic_folder,
            tint = if (open) MaterialTheme.colorScheme.onSurfaceVariant else faint,
        )
        // The name takes the room left, so the buttons line up at the end.
        Row(
            Modifier.weight(1f),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                project.name,
                modifier = Modifier.weight(1f, fill = false),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
                color = if (open) MaterialTheme.colorScheme.onSurface
                else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                project.sessions.toString(),
                modifier = Modifier.semantics { contentDescription = counted(project.sessions, "session") },
                style = MaterialTheme.typography.labelMedium,
                color = faint,
            )
        }
        // A closed project still shows which of its sessions need a look.
        if (!open) SignalDots(signals)
        pause?.invoke()
        onNewSession?.let { start ->
            IconButton(onClick = start, modifier = Modifier.size(48.dp)) {
                Icon(
                    painterResource(R.drawable.ic_add),
                    contentDescription = "New session in ${project.name}",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(18.dp),
                )
            }
        }
        menu?.invoke()
    }
}

/** "1 session", "3 sessions": a bare count, said with what it counts. */
private fun counted(count: Int, noun: String): String = "$count $noun${if (count == 1) "" else "s"}"

/**
 * A mark for each of up to five sessions that need a look; quiet ones draw
 * nothing. Each state has its own shape, and a screen reader hears the count
 * of each instead of the marks one by one.
 */
@Composable
private fun SignalDots(signals: List<SessionSignal>) {
    // A paused session is quiet until resumed; the header's Resume button says so.
    val shown = signals.filter { it != SessionSignal.IDLE && it != SessionSignal.PAUSED }
    if (shown.isEmpty()) return
    val summary = signalSummary(shown)
    Row(
        modifier = Modifier.clearAndSetSemantics { contentDescription = summary },
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        shown.take(5).forEach { signal ->
            val mark = signal.mark
            if (mark == null) BackgroundRing(signal.color(), size = 8.dp)
            else StatusMarkIcon(mark, labelled = false, still = true)
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
/** How far a session another session's agent started sits in from that one. */
private const val SESSION_NEST_INDENT = 12

private const val FOLDER_INDENT = 16

/** Told when New folder is offered by a host that would make a project instead. */
private const val NEEDS_NEWER_HOST = "Needs a newer xd host"

/**
 * An account's avatar, or [fallback] until the host has one to show. The
 * picture is decoded once per account, off the main thread and at about the
 * size it is drawn, and kept: a row scrolled back into view finds it ready.
 */
@Composable
internal fun OwnerAvatar(icon: ByteArray?, fallback: Int, tint: Color, size: Dp = 16.dp) {
    val key = remember(icon) { icon?.let(::avatarKey) }
    var bitmap by remember(key) { mutableStateOf(key?.let(decodedAvatars::get)) }
    if (icon != null && key != null && bitmap == null) {
        LaunchedEffect(key) {
            bitmap = withContext(Dispatchers.Default) { decodeAvatar(icon) }?.also { decodedAvatars.put(key, it) }
        }
    }
    val shown = bitmap
    if (shown != null) {
        Image(
            bitmap = shown,
            contentDescription = null,
            contentScale = ContentScale.Crop,
            modifier = Modifier.size(size).clip(MaterialTheme.shapes.extraSmall),
        )
    } else {
        Icon(painterResource(fallback), contentDescription = null, tint = tint, modifier = Modifier.size(size))
    }
}

/** The most pixels an avatar is drawn across: 24dp on the densest screens. */
private const val AVATAR_SIDE = 96

/** Decoded avatars by their bytes' fingerprint, so the same picture sent again is not decoded again. */
private val decodedAvatars = android.util.LruCache<String, ImageBitmap>(64)

/** Tells one avatar's bytes from another's: the same picture always has the same key. */
internal fun avatarKey(bytes: ByteArray): String = "${bytes.size}:${bytes.contentHashCode()}"

/**
 * Decodes an avatar no larger than it needs to be: the host sends them at
 * hundreds of pixels a side, and they are drawn at a few dozen.
 */
internal fun decodeAvatar(bytes: ByteArray): ImageBitmap? = runCatching {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (minOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= AVATAR_SIDE) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
}.getOrNull()

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

/**
 * The page's New button and the menu it opens above itself: a session when there is a
 * project to hold one, a project, or a folder, as the desktop's + offers. New session sits
 * last, nearest the button, and asks which project when there is more than one.
 */
@Composable
private fun NewMenu(
    canStartSession: Boolean,
    canCreateFolders: Boolean,
    onSession: () -> Unit,
    onProject: () -> Unit,
    onFolder: () -> Unit,
    button: @Composable (open: () -> Unit) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        button { open = true }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(
                text = { MenuLabel("New folder", NEEDS_NEWER_HOST.takeUnless { canCreateFolders }) },
                leadingIcon = { Icon(painterResource(R.drawable.ic_folder), contentDescription = null) },
                enabled = canCreateFolders,
                onClick = { open = false; onFolder() },
            )
            DropdownMenuItem(
                text = { Text("New project") },
                leadingIcon = { Icon(painterResource(R.drawable.ic_projects), contentDescription = null) },
                onClick = { open = false; onProject() },
            )
            if (canStartSession) {
                DropdownMenuItem(
                    text = { Text("New session") },
                    leadingIcon = { Icon(painterResource(R.drawable.ic_sessions), contentDescription = null) },
                    onClick = { open = false; onSession() },
                )
            }
        }
    }
}

/** A folder of projects: one line that opens and closes what it holds. */
@Composable
private fun FolderRow(
    folder: MinimalFolder,
    open: Boolean,
    icon: ByteArray?,
    /** What the sessions of every project inside need, shown while it is closed. */
    signals: List<SessionSignal>,
    toggle: () -> Unit,
    modifier: Modifier = Modifier,
    menu: @Composable () -> Unit,
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(MaterialTheme.shapes.small)
            .clickable(
                role = Role.Button,
                onClickLabel = if (open) "Close folder" else "Open folder",
                onClick = toggle,
            )
            .semantics { stateDescription = if (open) "Expanded" else "Collapsed" }
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
        Row(
            Modifier.weight(1f),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                folder.name,
                modifier = Modifier.weight(1f, fill = false),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                folder.items.toString(),
                modifier = Modifier.semantics { contentDescription = counted(folder.items, "item") },
                style = MaterialTheme.typography.labelMedium,
                color = faint,
            )
        }
        // A closed folder shows which of the sessions inside need a look.
        if (!open) SignalDots(signals)
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
            .clip(MaterialTheme.shapes.small)
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

/** An open project, with one row per session beneath it. */
@Composable
private fun OpenDrawer(
    project: MinimalProject,
    icon: ByteArray?,
    sessions: List<MinimalSession>,
    /** New sessions sent to the host and not made yet. */
    starting: List<StartingSession>,
    signalOf: (MinimalSession) -> SessionSignal,
    /** What a session shows on a folded row above it. */
    outwardOf: (MinimalSession) -> SessionSignal,
    open: (MinimalSession) -> Unit,
    actions: SessionActions,
    /** Sessions unfolded to show the sessions their agents started; the rest list folded. */
    expanded: Set<String>,
    fold: (MinimalSession, Boolean) -> Unit,
    /** Closes the project. */
    toggle: () -> Unit,
    onNewSession: () -> Unit,
    pause: (@Composable () -> Unit)?,
    menu: @Composable () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        DrawerRow(
            project = project,
            open = true,
            signals = emptyList(),
            onClick = toggle,
            icon = icon,
            onNewSession = onNewSession,
            pause = pause,
            menu = menu,
        )
        starting.forEach { session ->
            key("starting-${session.id}") { StartingRow(session) }
        }
        // Kept to one quiet line: several empty projects can be open at once.
        if (sessions.isEmpty() && starting.isEmpty()) {
            Text(
                "No sessions yet",
                modifier = Modifier.padding(start = 30.dp, end = 10.dp, bottom = 6.dp),
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        // A folded session hides the run of sessions below it that its agent started,
        // and every such session starts folded until the user unfolds it.
        var hiddenUntil = 0
        sessions.forEachIndexed { index, session ->
            if (index < hiddenUntil) return@forEachIndexed
            val workers = sessions.startedBelow(index)
            val isFolded = workers > 0 && session.id !in expanded
            if (isFolded) hiddenUntil = index + 1 + workers
            // Keyed so a row's open menu stays with its session when others come and go.
            key(session.id) {
                SessionRow(
                    session = session,
                    signal = signalOf(session),
                    onClick = { open(session) },
                    actions = actions,
                    fold = if (workers == 0) null else SessionFold(
                        workers = workers,
                        folded = isFolded,
                        hidden = if (isFolded) sessions.subList(index + 1, hiddenUntil).map(outwardOf) else emptyList(),
                        toggle = { fold(session, !isFolded) },
                    ),
                )
            }
        }
    }
}

/** A new session the host is still setting up, laid out as its row will be. */
@Composable
private fun StartingRow(session: StartingSession) {
    val signals = LocalXdSignals.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 18.dp)
            .heightIn(min = 64.dp)
            .padding(start = 12.dp, end = 16.dp)
            .semantics(mergeDescendants = true) {},
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
            Text(
                "Starting…",
                style = MaterialTheme.typography.labelMedium,
                color = signals.working,
                maxLines = 1,
            )
        }
    }
}

/** What a session row shows of the sessions its agent started, which list right below it. */
private class SessionFold(
    /** How many sessions below it its agent started, or theirs did. */
    val workers: Int,
    val folded: Boolean,
    /** The signals of the sessions folding hides, so the row still shows which need a look. */
    val hidden: List<SessionSignal>,
    val toggle: () -> Unit,
)

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
    fold: SessionFold? = null,
) {
    val signals = LocalXdSignals.current
    // With enlarged text the words of a status would squeeze the title out of
    // the row, so they move to the line under it and the mark stays at the end.
    val wordsBelow = LocalDensity.current.fontScale > 1.3f &&
        (signal == SessionSignal.WAITING || signal == SessionSignal.PAUSED)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 18.dp)
            .then(
                // Started by the agent of a session above: indented under it,
                // along a guide line for each level, as on the desktop.
                (0 until session.depth).fold<Int, Modifier>(Modifier) { modifier, _ ->
                    modifier
                        .padding(start = SESSION_NEST_INDENT.dp)
                        .drawBehind {
                            drawLine(
                                signals.rule,
                                Offset(0f, 0f),
                                Offset(0f, size.height),
                                strokeWidth = 1.dp.toPx(),
                            )
                        }
                        .padding(start = 4.dp)
                },
            )
            .heightIn(min = 64.dp)
            .clip(MaterialTheme.shapes.small)
            .clickable(role = Role.Button, onClickLabel = "Open session", onClick = onClick)
            .padding(start = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        BackendIcon(session.backend, size = 15.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                session.title,
                style = MaterialTheme.typography.bodyLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (wordsBelow) Text(
                    signal.label,
                    style = MaterialTheme.typography.labelMedium,
                    fontWeight = FontWeight.Medium,
                    color = signal.color(),
                    maxLines = 1,
                )
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
        // Folded, it says how many sessions it hides and which of them need a
        // look, the way a closed project does.
        if (fold?.folded == true) {
            Text(
                "${fold.workers}",
                modifier = Modifier.semantics { contentDescription = counted(fold.workers, "agent session") },
                style = MaterialTheme.typography.labelMedium,
                color = signals.faint,
            )
            SignalDots(fold.hidden)
        }
        SignalMark(signal, backgroundTasks = session.backgroundTasks, wordless = wordsBelow)
        if (fold != null) {
            IconButton(onClick = fold.toggle, modifier = Modifier.size(48.dp)) {
                Icon(
                    painterResource(if (fold.folded) R.drawable.ic_chevron_right else R.drawable.ic_chevron_down),
                    contentDescription = if (fold.folded) {
                        "Show the agents ${session.title} started"
                    } else {
                        "Hide the agents ${session.title} started"
                    },
                    tint = signals.faint,
                    modifier = Modifier.size(16.dp),
                )
            }
        }
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
                .clip(MaterialTheme.shapes.small)
                .disclosure(expanded, onToggle = toggle)
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
            Text(
                settled.size.toString(),
                modifier = Modifier.semantics { contentDescription = counted(settled.size, "session") },
                style = MaterialTheme.typography.labelMedium,
                color = faint,
            )
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

@Composable
private fun AgentPill(agent: DirectAgent) {
    Row(
        modifier = Modifier
            .clip(CircleShape)
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
            .clip(MaterialTheme.shapes.medium)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, MaterialTheme.shapes.medium)
            .padding(20.dp),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
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
 * The theme choices as cards with the swatch over the name, like the
 * desktop's: all in one row when the names fit, else two to a row. When even
 * that cannot fit them, as on a narrow phone with enlarged text, the choices
 * stack as full-width rows instead.
 */
@Composable
internal fun ThemePicker(theme: MinimalThemePreset, choose: (MinimalThemePreset) -> Unit) {
    val presets = MinimalThemePreset.entries
    val measurer = rememberTextMeasurer()
    val style = LocalTextStyle.current.copy(fontWeight = FontWeight.SemiBold)
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val gap = 8.dp
        val padding = 10.dp
        val widest = with(LocalDensity.current) {
            presets.maxOf { measurer.measure(it.label, style).size.width }.toDp()
        }
        val fits = { columns: Int -> (maxWidth - gap * (columns - 1)) / columns - padding * 2 >= widest }
        val columns = listOf(presets.size, 2).firstOrNull(fits) ?: 1
        val card = columns > 1
        Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(gap)) {
            presets.chunked(columns).forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                    row.forEach { preset ->
                        ThemeChoice(
                            preset,
                            chosen = theme == preset,
                            card = card,
                            padding = padding,
                            modifier = Modifier.weight(1f),
                        ) { choose(preset) }
                    }
                    repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

/** The theme's ground at a glance: its glass backdrop under a frosted strip. */
@Composable
private fun ThemeSwatch(preset: MinimalThemePreset, modifier: Modifier) {
    val glass = xdGlass(preset)
    val shape = MaterialTheme.shapes.extraSmall
    Box(
        modifier
            .clip(shape)
            .background(glass.backdrop)
            .border(1.dp, MaterialTheme.colorScheme.outline, shape),
    ) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(0.35f).background(glass.pane))
    }
}

@Composable
private fun ThemeChoice(
    preset: MinimalThemePreset,
    chosen: Boolean,
    card: Boolean,
    padding: Dp,
    modifier: Modifier,
    choose: () -> Unit,
) {
    val shape = MaterialTheme.shapes.small
    val frame = modifier
        .heightIn(min = 48.dp)
        .clip(shape)
        .border(
            if (chosen) 1.5.dp else 1.dp,
            if (chosen) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
            shape,
        )
        .selectable(selected = chosen, role = Role.RadioButton, onClick = choose)
        .padding(padding)
    val label: @Composable () -> Unit = {
        Text(
            preset.label,
            fontWeight = if (chosen) FontWeight.SemiBold else FontWeight.Normal,
            textAlign = TextAlign.Center,
        )
    }
    if (card) {
        Column(
            frame,
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            ThemeSwatch(preset, Modifier.fillMaxWidth().height(20.dp))
            label()
        }
    } else {
        Row(
            frame,
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            ThemeSwatch(preset, Modifier.size(width = 28.dp, height = 20.dp))
            label()
        }
    }
}

/**
 * The host's rules for settling sessions by itself, with the desktop's
 * wording. They belong to the host, so they keep running while no client is
 * connected.
 */
@Composable
internal fun AutoSettleSettings(model: MainViewModel) {
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
        title = "Settle merged sessions",
        detail = SETTLE_MERGED_DETAIL,
        checked = current.onMerge,
        onCheckedChange = { model.setAutoSettle(onMerge = it) },
        enabled = editable,
    )
    SettingSwitch(
        title = "Settle inactive sessions",
        detail = SETTLE_INACTIVE_DETAIL,
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
                Text("Days before settling", fontWeight = FontWeight.SemiBold)
                Text(
                    SETTLE_DAYS_DETAIL,
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
        title = "Keep sessions with a pull request",
        detail = SETTLE_KEEP_PR_DETAIL,
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
internal fun SettingSwitch(
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
internal fun SettingAction(
    title: String,
    detail: String,
    action: String,
    /** The action removes something that cannot be brought back with a tap. */
    destructive: Boolean = false,
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
        TextButton(
            onClick = onClick,
            colors = if (destructive) ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)
            else ButtonDefaults.textButtonColors(),
        ) { Text(action) }
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
