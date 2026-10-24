@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.width
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
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
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

private data class SessionDestination(
    val projectId: String,
    val projectName: String,
    val chatId: String,
    val title: String,
    val agent: DirectAgent,
)

private enum class MobileDestination {
    PROJECTS,
    SESSIONS,
    TERMINAL,
}

private const val GLOBAL_TERMINAL_CHAT_ID = "global:mobile"

private val SessionTabsSaver = listSaver<List<SessionDestination>, String>(
    save = { tabs ->
        tabs.flatMap { tab ->
            listOf(tab.projectId, tab.projectName, tab.chatId, tab.title, tab.agent.name)
        }
    },
    restore = { saved ->
        saved.chunked(5).map { fields ->
            SessionDestination(
                projectId = fields[0],
                projectName = fields[1],
                chatId = fields[2],
                title = fields[3],
                agent = DirectAgent.valueOf(fields[4]),
            )
        }
    },
)

@Composable
internal fun MinimalMobileApp(
    model: MainViewModel,
    link: Link,
    settings: MobileSettings,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val created by model.createdDirectSession.collectAsStateWithLifecycle()
    var tabs by rememberSaveable(stateSaver = SessionTabsSaver) {
        mutableStateOf(emptyList())
    }
    var activeChatId by rememberSaveable { mutableStateOf<String?>(null) }
    var mobileDestination by rememberSaveable {
        mutableStateOf(MobileDestination.PROJECTS)
    }
    var createTabFor by remember { mutableStateOf<Pair<String, String>?>(null) }
    val projects = tree.minimalProjects()
    val knownDestinations = projects.flatMap { project ->
        tree.minimalSessions(project.id).map { session ->
            SessionDestination(
                projectId = project.id,
                projectName = project.name,
                chatId = session.id,
                title = session.title,
                agent = session.agent,
            )
        }
    }

    LaunchedEffect(knownDestinations) {
        val current = knownDestinations.associateBy(SessionDestination::chatId)
        tabs = tabs.map { current[it.chatId] ?: it }
    }

    LaunchedEffect(created, projects) {
        created?.let { session ->
            val destination = SessionDestination(
                projectId = session.projectId,
                projectName = projects.firstOrNull { it.id == session.projectId }?.name ?: "Project",
                chatId = session.chatId,
                title = session.title,
                agent = session.agent,
            )
            tabs = tabs.filterNot { it.chatId == destination.chatId } + destination
            activeChatId = destination.chatId
            mobileDestination = MobileDestination.SESSIONS
            model.consumeCreatedDirectSession(session)
        }
    }

    val active = tabs.firstOrNull { it.chatId == activeChatId }
    val showProjects = { mobileDestination = MobileDestination.PROJECTS }
    val showSessions = {
        val destination = active ?: knownDestinations.firstOrNull()
        if (destination != null) {
            tabs = tabs.filterNot { it.chatId == destination.chatId } + destination
            activeChatId = destination.chatId
            mobileDestination = MobileDestination.SESSIONS
        }
    }
    val showTerminal = { mobileDestination = MobileDestination.TERMINAL }
    val closeTab: (String) -> Unit = { chatId ->
        val closingIndex = tabs.indexOfFirst { it.chatId == chatId }
        val remaining = tabs.filterNot { it.chatId == chatId }
        if (activeChatId == chatId) {
            activeChatId = if (remaining.isEmpty()) {
                null
            } else {
                remaining[closingIndex.coerceAtMost(remaining.lastIndex)].chatId
            }
        }
        tabs = remaining
        if (remaining.isEmpty()) mobileDestination = MobileDestination.PROJECTS
    }

    val imeVisible = WindowInsets.isImeVisible
    Column(Modifier.fillMaxSize()) {
    Box(Modifier.weight(1f)) {
    when {
        mobileDestination == MobileDestination.TERMINAL -> MinimalGlobalTerminal(
            model = model,
            settings = settings,
            link = link,
            onProjects = showProjects,
            onSessions = showSessions,
        )

        mobileDestination == MobileDestination.SESSIONS && active != null -> {
            key(active.chatId) {
                if (tree.chats.firstOrNull { it.id == active.chatId }?.sessionMode == "chat") {
                    val owner = remember(active.chatId) { DirectTerminalOwner() }
                    DisposableEffect(owner) { onDispose { owner.viewModelStore.clear() } }
                    val chat: ChatViewModel = viewModel(key = "chat-${active.chatId}", viewModelStoreOwner = owner, factory = ChatViewModel.Factory(model.client, active.chatId))
                    ChatScreen(model = chat, settings = settings, goBack = showProjects,
                        productHeader = {},
                        sessionTabs = { SessionTabStrip(tabs = tabs, selectedChatId = active.chatId, selectTab = { activeChatId = it }, closeTab = closeTab, addTab = { createTabFor = active.projectId to active.projectName }) },
                    )
                } else {
                MinimalDirectSession(
                    model = model,
                    settings = settings,
                    link = link,
                    destination = active,
                    tabs = tabs,
                    selectTab = { activeChatId = it },
                    closeTab = closeTab,
                    addTab = {
                        createTabFor = active.projectId to active.projectName
                    },
                    onProjects = showProjects,
                    onTerminal = showTerminal,
                )
                }
            }
        }

        else -> MinimalProjectsHome(
            model = model,
            settings = settings,
            link = link,
            projects = projects,
            loading = tree.loading,
            treeError = tree.error,
            onSessions = showSessions,
            onTerminal = showTerminal,
            open = { project, session ->
                val destination = SessionDestination(
                    projectId = project.id,
                    projectName = project.name,
                    chatId = session.id,
                    title = session.title,
                    agent = session.agent,
                )
                tabs = tabs.filterNot { it.chatId == destination.chatId } + destination
                activeChatId = destination.chatId
                mobileDestination = MobileDestination.SESSIONS
                model.client.markSeen(session.id)
            },
        )
    }
    }
    if (!imeVisible) {
        XdNavigationBar(
            active = mobileDestination,
            onProjects = showProjects,
            onSessions = showSessions,
            onTerminal = showTerminal,
        )
    }
    }

    createTabFor?.let { (projectId, projectName) ->
        NewSessionDialog(
            projectName = projectName,
            onDismiss = { createTabFor = null },
            onCreate = { title, agent ->
                createTabFor = null
                model.createDirectSession(projectId, title, agent)
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
    open: (MinimalProject, MinimalSession) -> Unit,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()
    var selectedId by rememberSaveable { mutableStateOf<String?>(null) }
    var createProject by rememberSaveable { mutableStateOf(false) }
    var createSession by rememberSaveable { mutableStateOf(false) }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }

    val selected = projects.firstOrNull { it.id == selectedId } ?: projects.firstOrNull()
    val sessions = selected?.let { tree.minimalSessions(it.id) }.orEmpty()
    val waiting by model.client.waitingChats.collectAsStateWithLifecycle()
    val unread by model.client.unreadChats.collectAsStateWithLifecycle()
    val signalOf = { session: MinimalSession ->
        when {
            session.id in waiting -> SessionSignal.WAITING
            session.working -> SessionSignal.WORKING
            session.id in unread -> SessionSignal.DONE
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
                        EmptyCard("Create a workspace, then start a Codex, Claude, JCode, or Copilot session.")
                    }
                } else {
                    items(projects, key = MinimalProject::id) { project ->
                        if (project.id == selected?.id) {
                            OpenDrawer(
                                project = project,
                                sessions = sessions,
                                signalOf = signalOf,
                                open = { session -> open(project, session) },
                            )
                        } else {
                            DrawerRow(
                                project = project,
                                selected = false,
                                signals = tree.minimalSessions(project.id).map(signalOf),
                                onClick = { selectedId = project.id },
                            )
                        }
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
private fun MinimalDirectSession(
    model: MainViewModel,
    settings: MobileSettings,
    link: Link,
    destination: SessionDestination,
    tabs: List<SessionDestination>,
    selectTab: (String) -> Unit,
    closeTab: (String) -> Unit,
    addTab: () -> Unit,
    onProjects: () -> Unit,
    onTerminal: () -> Unit,
) {
    val allowAllPermissions by settings.allowAllPermissions.collectAsStateWithLifecycle()
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    val terminalOwner = remember(
        destination.chatId,
        destination.agent,
        allowAllPermissions,
    ) { DirectTerminalOwner() }
    DisposableEffect(terminalOwner) {
        onDispose { terminalOwner.viewModelStore.clear() }
    }
    val terminal: TerminalViewModel = viewModel(
        key = "direct-${destination.chatId}-${destination.agent.wire}-$allowAllPermissions",
        viewModelStoreOwner = terminalOwner,
        factory = TerminalViewModel.DirectFactory(
            client = model.client,
            chatId = destination.chatId,
            agent = destination.agent,
            allowAllPermissions = allowAllPermissions,
        ),
    )
    LaunchedEffect(terminal) {
        model.client.terminalEvents.collect(terminal::onEvent)
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .navigationBarsPadding(),
    ) {
        ProductHeader(link = link, title = destination.projectName, onSettings = { settingsOpen = true })
        SessionTabStrip(
            tabs = tabs,
            selectedChatId = destination.chatId,
            selectTab = selectTab,
            closeTab = closeTab,
            addTab = addTab,
        )
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .height(54.dp)
                .background(MaterialTheme.colorScheme.surfaceContainerLow)
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            TextButton(onClick = onProjects, contentPadding = PaddingValues(4.dp)) {
                Text("←", fontSize = 22.sp)
            }
            Column(Modifier.weight(1f)) {
                Text(
                    destination.projectName,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    destination.title,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            AgentPill(destination.agent)
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
    val terminalOwner = remember { DirectTerminalOwner() }
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
    LaunchedEffect(terminal) {
        model.client.terminalEvents.collect(terminal::onEvent)
    }

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
private fun SessionTabStrip(
    tabs: List<SessionDestination>,
    selectedChatId: String,
    selectTab: (String) -> Unit,
    closeTab: (String) -> Unit,
    addTab: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(46.dp)
            .horizontalScroll(rememberScrollState())
            .background(MaterialTheme.colorScheme.surface),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        tabs.forEach { tab ->
            val selected = tab.chatId == selectedChatId
            Row(
                modifier = Modifier
                    .height(46.dp)
                    .background(
                        if (selected) MaterialTheme.colorScheme.surfaceContainerHigh
                        else MaterialTheme.colorScheme.surface,
                    )
                    .clickable { selectTab(tab.chatId) }
                    .padding(start = 13.dp, end = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(7.dp),
            ) {
                BackendIcon(tab.agent.wire, size = 15.dp)
                Text(
                    tab.title,
                    modifier = Modifier.widthIn(max = 126.dp),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontSize = 12.sp,
                    fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
                )
                Box(
                    modifier = Modifier
                        .size(40.dp)
                        .clip(CircleShape)
                        .clickable { closeTab(tab.chatId) },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        painterResource(R.drawable.ic_close),
                        contentDescription = "Close ${tab.title}",
                        tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.size(16.dp),
                    )
                }
            }
        }
        Box(
            modifier = Modifier
                .size(46.dp)
                .clickable(onClick = addTab),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painterResource(R.drawable.ic_add),
                contentDescription = "New tab",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
        Box(
            modifier = Modifier
                .size(24.dp)
                .clip(RoundedCornerShape(6.dp))
                .background(MaterialTheme.colorScheme.primary),
            contentAlignment = Alignment.Center,
        ) {
            Text("x", color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.Bold)
        }
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
 * A tab in the state's own shape as well as colour: working is solid, waiting
 * is a taller solid flag, done is hollow.
 */
@Composable
private fun SignalTab(signal: SessionSignal, width: Dp, height: Dp, modifier: Modifier = Modifier) {
    if (signal == SessionSignal.IDLE) return
    val color = signal.color()
    val shape = RoundedCornerShape(topStart = 2.dp, topEnd = 2.dp)
    Box(
        modifier
            .size(width = width, height = if (signal == SessionSignal.WAITING) height * 1.4f else height)
            .clip(shape)
            .then(
                if (signal == SessionSignal.DONE) Modifier.border(1.5.dp, color, shape)
                else Modifier.background(color),
            ),
    )
}

/** A closed drawer: one line, with a tab for each session that is not idle. */
@Composable
private fun DrawerRow(
    project: MinimalProject,
    selected: Boolean,
    signals: List<SessionSignal>,
    onClick: () -> Unit,
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(
            painterResource(if (selected) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
            contentDescription = null,
            tint = faint,
            modifier = Modifier.size(16.dp),
        )
        if (selected) {
            Text(
                project.name,
                modifier = Modifier
                    .weight(1f, fill = false)
                    .border(1.5.dp, MaterialTheme.colorScheme.primary, RoundedCornerShape(3.dp))
                    .padding(horizontal = 8.dp, vertical = 2.dp),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        } else {
            Text(
                project.name,
                modifier = Modifier.weight(1f, fill = false),
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(
            project.sessions.toString(),
            fontFamily = XdMonoFamily,
            fontSize = 12.sp,
            color = faint,
        )
        Spacer(Modifier.weight(1f))
        if (!selected) {
            Row(
                horizontalArrangement = Arrangement.spacedBy(3.dp),
                verticalAlignment = Alignment.Bottom,
            ) {
                signals.filter { it != SessionSignal.IDLE }.take(6).forEach { signal ->
                    SignalTab(signal, width = 10.dp, height = 5.dp)
                }
            }
        }
    }
}

/** The current project's drawer, lifted out with its session cards. */
@Composable
private fun OpenDrawer(
    project: MinimalProject,
    sessions: List<MinimalSession>,
    signalOf: (MinimalSession) -> SessionSignal,
    open: (MinimalSession) -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 6.dp)
            .clip(RoundedCornerShape(10.dp))
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.background)
            .padding(6.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        DrawerRow(project = project, selected = true, signals = emptyList(), onClick = {})
        if (sessions.isEmpty()) {
            Text(
                "No sessions yet. Start one with Codex, Claude, JCode, or Copilot.",
                modifier = Modifier.padding(horizontal = 10.dp, vertical = 6.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        sessions.forEach { session ->
            SessionRow(session = session, signal = signalOf(session), onClick = { open(session) })
        }
    }
}

/**
 * A session's index card: title over the red ruled line, then branch and
 * state. The tab clipped to the top edge says what its agent is doing.
 */
@Composable
private fun SessionRow(session: MinimalSession, signal: SessionSignal, onClick: () -> Unit) {
    val signals = LocalXdSignals.current
    Box(Modifier.fillMaxWidth().padding(top = 6.dp)) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(7.dp))
                .background(MaterialTheme.colorScheme.surfaceContainer)
                .clickable(onClick = onClick)
                .padding(horizontal = 14.dp, vertical = 11.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                BackendIcon(session.agent.wire, size = 17.dp)
                Text(
                    session.title,
                    modifier = Modifier
                        .weight(1f)
                        .padding(end = 18.dp),
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Box(
                Modifier
                    .padding(vertical = 9.dp)
                    .fillMaxWidth()
                    .height(1.dp)
                    .background(signals.rule),
            )
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Text(
                    session.branch,
                    modifier = Modifier.weight(1f, fill = false),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    // Branch names are code; the placeholder for none is prose.
                    fontFamily = if (session.branch == "Project directory") null else XdMonoFamily,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text("·", color = signals.faint, fontSize = 12.sp)
                if (signal == SessionSignal.WAITING) {
                    Text(
                        signal.label.uppercase(),
                        modifier = Modifier
                            .border(1.dp, signal.color(), RoundedCornerShape(2.dp))
                            .padding(horizontal = 5.dp, vertical = 1.dp),
                        color = signal.color(),
                        style = MaterialTheme.typography.labelSmall,
                        fontWeight = FontWeight.SemiBold,
                        letterSpacing = 0.6.sp,
                    )
                } else {
                    Text(
                        signal.label,
                        color = signal.color(),
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
                if (signal == SessionSignal.WORKING) WorkingDots()
            }
        }
        SignalTab(
            signal,
            width = 18.dp,
            height = 10.dp,
            modifier = Modifier
                .align(Alignment.TopEnd)
                .padding(end = 16.dp)
                .offset(y = (-4).dp),
        )
    }
}

@Composable
private fun AgentPill(agent: DirectAgent) {
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surface)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(horizontal = 9.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        BackendIcon(agent.wire, size = 15.dp)
        Text(agent.wire, fontFamily = XdMonoFamily, fontSize = 11.sp)
    }
}

@Composable
private fun WorkingDots() {
    Dots(LocalXdSignals.current.working, contentDescription = "Working")
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
            .clip(RoundedCornerShape(8.dp))
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(8.dp))
            .padding(20.dp),
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun NewSessionDialog(
    projectName: String,
    onDismiss: () -> Unit,
    onCreate: (String, DirectAgent) -> Unit,
) {
    var title by rememberSaveable { mutableStateOf("") }
    var agent by rememberSaveable { mutableStateOf(DirectAgent.CODEX) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New session") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(projectName, color = MaterialTheme.colorScheme.onSurfaceVariant)
                OutlinedTextField(
                    value = title,
                    onValueChange = { title = it },
                    label = { Text("What are you working on?") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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
            Button(onClick = { onCreate(title.trim(), agent) }) { Text("Create") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun NewProjectDialog(
    onDismiss: () -> Unit,
    onCreate: (String, String?) -> Unit,
) {
    var name by rememberSaveable { mutableStateOf("") }
    var repository by rememberSaveable { mutableStateOf("") }
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
                )
            }
        },
        confirmButton = {
            Button(
                onClick = { onCreate(name.trim(), repository.trim().ifBlank { null }) },
                enabled = name.isNotBlank(),
            ) { Text("Create") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun MinimalSettingsDialog(
    settings: MobileSettings,
    onDismiss: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val theme by settings.theme.collectAsStateWithLifecycle()
    val allPermissions by settings.allowAllPermissions.collectAsStateWithLifecycle()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
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
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("All permissions", fontWeight = FontWeight.SemiBold)
                        Text(
                            "Pass the agent's unrestricted command-line flag.",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    Switch(
                        checked = allPermissions,
                        onCheckedChange = settings::setAllowAllPermissions,
                    )
                }
                OutlinedButton(onClick = onDisconnect, modifier = Modifier.fillMaxWidth()) {
                    Text("Disconnect remote")
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

private class DirectTerminalOwner : ViewModelStoreOwner {
    override val viewModelStore: ViewModelStore = ViewModelStore()
}
