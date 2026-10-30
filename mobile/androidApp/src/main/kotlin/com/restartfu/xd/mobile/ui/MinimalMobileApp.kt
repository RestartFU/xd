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
        saved.chunked(5).mapNotNull { fields ->
            if (fields.size != 5) return@mapNotNull null
            val agent = DirectAgent.entries.firstOrNull { it.name == fields[4] }
                ?: return@mapNotNull null
            SessionDestination(
                projectId = fields[0],
                projectName = fields[1],
                chatId = fields[2],
                title = fields[3],
                agent = agent,
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

    LaunchedEffect(knownDestinations, tree.loading, tree.error) {
        // Wait for the host's workspace snapshot before pruning restored tabs.
        if (!tree.loading && tree.error == null && tree.folders.isNotEmpty()) {
            val current = knownDestinations.associateBy(SessionDestination::chatId)
            tabs = tabs.mapNotNull { current[it.chatId] }
            if (tabs.none { it.chatId == activeChatId }) activeChatId = tabs.firstOrNull()?.chatId
        }
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
                val owner = remember(active.chatId) { SessionViewModelOwner() }
                DisposableEffect(owner) { onDispose { owner.viewModelStore.clear() } }
                val chat: ChatViewModel = viewModel(key = "chat-${active.chatId}", viewModelStoreOwner = owner, factory = ChatViewModel.Factory(model.client, active.chatId))
                ChatScreen(model = chat, settings = settings, goBack = showProjects,
                    productHeader = {},
                    sessionTabs = { SessionTabStrip(tabs = tabs, selectedChatId = active.chatId, selectTab = { activeChatId = it }, closeTab = closeTab, addTab = { createTabFor = active.projectId to active.projectName }) },
                )
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
) {
    val faint = LocalXdSignals.current.faint
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
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

/** The current project, open, with one row per session beneath it. */
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
            .padding(vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        DrawerRow(project = project, selected = true, signals = emptyList(), onClick = {})
        if (sessions.isEmpty()) {
            Text(
                "No sessions yet. Start one with Codex, Claude, or JCode.",
                modifier = Modifier.padding(start = 30.dp, end = 10.dp, top = 4.dp, bottom = 8.dp),
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
 * One session, on one line: its agent, its title, and a status mark only when
 * there is something to say. A branch shows when the session has its own.
 */
@Composable
private fun SessionRow(session: MinimalSession, signal: SessionSignal, onClick: () -> Unit) {
    val signals = LocalXdSignals.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 18.dp)
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        BackendIcon(session.agent.wire, size = 15.dp)
        Text(
            session.title,
            modifier = Modifier.weight(1f),
            style = MaterialTheme.typography.bodyLarge,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (signal == SessionSignal.IDLE && session.branch != "Project directory") {
            Text(
                session.branch,
                modifier = Modifier.widthIn(max = 110.dp),
                color = signals.faint,
                fontFamily = XdMonoFamily,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        SignalMark(signal)
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

private class SessionViewModelOwner : ViewModelStoreOwner {
    override val viewModelStore: ViewModelStore = ViewModelStore()
}
