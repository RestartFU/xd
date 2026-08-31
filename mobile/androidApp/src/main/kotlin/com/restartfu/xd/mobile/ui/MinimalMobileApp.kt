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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
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
    val experimentMode by settings.experimentMode.collectAsStateWithLifecycle()
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

    when {
        mobileDestination == MobileDestination.TERMINAL -> MinimalGlobalTerminal(
            model = model,
            settings = settings,
            link = link,
            onProjects = showProjects,
            onSessions = showSessions,
        )

        mobileDestination == MobileDestination.SESSIONS && active != null -> {
            key(experimentMode, active.chatId) {
                if (experimentMode) {
                    MinimalNativeSession(
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
            },
        )
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

    LaunchedEffect(selected?.id) {
        if (selectedId != selected?.id) selectedId = selected?.id
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .navigationBarsPadding(),
    ) {
        ProductHeader(
            link = link,
            active = MobileDestination.PROJECTS,
            onProjects = {},
            onSessions = {
                val session = sessions.firstOrNull()
                if (selected != null && session != null) open(selected, session) else onSessions()
            },
            onTerminal = onTerminal,
            onAdd = { createProject = true },
            onSettings = { settingsOpen = true },
        )
        ContextBar(title = "Projects")

        when {
            loading && projects.isEmpty() -> Box(
                Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }

            else -> LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(16.dp, 18.dp, 16.dp, 28.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                (treeError ?: operationError)?.let { error ->
                    item("error") {
                        Text(error, color = MaterialTheme.colorScheme.error)
                    }
                }
                item("projects-label") { SectionLabel("PROJECTS") }
                if (projects.isEmpty()) {
                    item("empty-projects") {
                        EmptyCard("Create a workspace, then start a Codex, Claude, JCode, or Copilot session.")
                    }
                } else {
                    items(projects, key = MinimalProject::id) { project ->
                        ProjectRow(
                            project = project,
                            selected = project.id == selected?.id,
                            onClick = { selectedId = project.id },
                        )
                    }
                }

                selected?.let { project ->
                    item("project-heading") {
                        Spacer(Modifier.height(14.dp))
                        SectionLabel("PROJECT")
                        Text(
                            project.name,
                            modifier = Modifier.padding(top = 3.dp),
                            color = MaterialTheme.colorScheme.onBackground,
                            fontSize = 28.sp,
                            fontWeight = FontWeight.Bold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 22.dp, bottom = 2.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                "Sessions",
                                modifier = Modifier.weight(1f),
                                fontWeight = FontWeight.SemiBold,
                            )
                            Button(onClick = { createSession = true }) {
                                Text("＋  New session")
                            }
                        }
                    }
                    if (sessions.isEmpty()) {
                        item("empty-sessions") {
                            EmptyCard("No sessions yet. Start one with Codex, Claude, JCode, or Copilot.")
                        }
                    } else {
                        items(sessions, key = MinimalSession::id) { session ->
                            SessionRow(session = session, onClick = { open(project, session) })
                        }
                    }
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
private fun MinimalNativeSession(
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
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    val chatOwner = remember(destination.chatId) { NativeChatOwner() }
    DisposableEffect(chatOwner) {
        onDispose { chatOwner.viewModelStore.clear() }
    }
    CompositionLocalProvider(LocalViewModelStoreOwner provides chatOwner) {
        val chat: ChatViewModel = viewModel(
            key = "chat-${destination.chatId}",
            factory = ChatViewModel.Factory(
                client = model.client,
                chatId = destination.chatId,
            ),
        )
        ChatScreen(
            model = chat,
            settings = settings,
            goBack = onProjects,
            productHeader = {
                ProductHeader(
                    link = link,
                    active = MobileDestination.SESSIONS,
                    onProjects = onProjects,
                    onSessions = {},
                    onTerminal = onTerminal,
                    onAdd = addTab,
                    onSettings = { settingsOpen = true },
                )
            },
            sessionTabs = {
                SessionTabStrip(
                    tabs = tabs,
                    selectedChatId = destination.chatId,
                    selectTab = selectTab,
                    closeTab = closeTab,
                    addTab = addTab,
                )
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
        ProductHeader(
            link = link,
            active = MobileDestination.SESSIONS,
            onProjects = onProjects,
            onSessions = {},
            onTerminal = onTerminal,
            onAdd = {},
            onSettings = { settingsOpen = true },
            showAdd = false,
        )
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
        ProductHeader(
            link = link,
            active = MobileDestination.TERMINAL,
            onProjects = onProjects,
            onSessions = onSessions,
            onTerminal = {},
            onAdd = {},
            onSettings = { settingsOpen = true },
            showAdd = false,
        )
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
                Text(
                    "×",
                    modifier = Modifier
                        .clip(CircleShape)
                        .clickable { closeTab(tab.chatId) }
                        .padding(horizontal = 7.dp, vertical = 5.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    fontSize = 16.sp,
                )
            }
        }
        Box(
            modifier = Modifier
                .size(46.dp)
                .clickable(onClick = addTab),
            contentAlignment = Alignment.Center,
        ) {
            Text("+", fontSize = 22.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

@Composable
private fun ProductHeader(
    link: Link,
    active: MobileDestination,
    onProjects: () -> Unit,
    onSessions: () -> Unit,
    onTerminal: () -> Unit,
    onAdd: () -> Unit,
    onSettings: () -> Unit,
    showAdd: Boolean = true,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface)
            .statusBarsPadding()
            .height(62.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            modifier = Modifier
                .size(22.dp)
                .clip(RoundedCornerShape(7.dp))
                .background(MaterialTheme.colorScheme.primary),
            contentAlignment = Alignment.Center,
        ) {
            Text("x", color = MaterialTheme.colorScheme.onPrimary, fontWeight = FontWeight.Black)
        }
        Text("xd", fontSize = 19.sp, fontWeight = FontWeight.Bold)
        Row(
            modifier = Modifier
                .weight(1f)
                .horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            NavPill(
                "Projects",
                active = active == MobileDestination.PROJECTS,
                onClick = onProjects,
            )
            NavPill(
                "Sessions",
                active = active == MobileDestination.SESSIONS,
                onClick = onSessions,
            )
            NavPill(
                "Terminal",
                active = active == MobileDestination.TERMINAL,
                onClick = onTerminal,
            )
            if (showAdd) {
                Box(
                    modifier = Modifier
                        .size(36.dp)
                        .clip(CircleShape)
                        .background(MaterialTheme.colorScheme.primary)
                        .clickable(onClick = onAdd),
                    contentAlignment = Alignment.Center,
                ) {
                    Text("+", color = MaterialTheme.colorScheme.onPrimary, fontSize = 22.sp)
                }
            }
        }
        Box(
            modifier = Modifier
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.background)
                .padding(horizontal = 9.dp, vertical = 7.dp),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(if (link is Link.Up) Color(0xFF36C75C) else Color(0xFFB74C58)),
            )
        }
        IconButton(onClick = onSettings, modifier = Modifier.size(36.dp)) {
            Icon(
                painter = painterResource(R.drawable.ic_settings),
                contentDescription = "Settings",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

@Composable
private fun NavPill(label: String, active: Boolean, onClick: () -> Unit) {
    Text(
        label,
        modifier = Modifier
            .clip(CircleShape)
            .background(
                if (active) MaterialTheme.colorScheme.surfaceContainerHigh
                else Color.Transparent,
            )
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 9.dp),
        color = if (active) MaterialTheme.colorScheme.onSurface
        else MaterialTheme.colorScheme.onSurfaceVariant,
        fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium,
        fontSize = 13.sp,
    )
}

@Composable
private fun ContextBar(title: String) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(50.dp)
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, fontWeight = FontWeight.SemiBold)
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontSize = 11.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.8.sp,
    )
}

@Composable
private fun ProjectRow(project: MinimalProject, selected: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(13.dp))
            .background(
                if (selected) MaterialTheme.colorScheme.primaryContainer
                else MaterialTheme.colorScheme.surface,
            )
            .border(
                1.dp,
                if (selected) MaterialTheme.colorScheme.primary
                else MaterialTheme.colorScheme.outlineVariant,
                RoundedCornerShape(13.dp),
            )
            .clickable(onClick = onClick)
            .padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("▰", color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
        Column(Modifier.weight(1f)) {
            Text(project.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                "${project.sessions} session${if (project.sessions == 1) "" else "s"}",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.labelSmall,
            )
        }
        if (project.working > 0) WorkingDots()
    }
}

@Composable
private fun SessionRow(session: MinimalSession, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(15.dp))
            .background(MaterialTheme.colorScheme.surface)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(15.dp))
            .clickable(onClick = onClick)
            .padding(15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(13.dp),
    ) {
        Box(
            Modifier
                .size(40.dp)
                .clip(RoundedCornerShape(11.dp))
                .background(MaterialTheme.colorScheme.background),
            contentAlignment = Alignment.Center,
        ) {
            BackendIcon(session.agent.wire, size = 22.dp)
        }
        Column(Modifier.weight(1f)) {
            Text(session.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    session.branch,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    "  ·  ${if (session.working) "Working" else "Idle"}",
                    color = if (session.working) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.labelSmall,
                )
            }
        }
        if (session.working) WorkingDots() else Text("›", color = MaterialTheme.colorScheme.onSurfaceVariant, fontSize = 22.sp)
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
        Text(agent.wire, fontFamily = FontFamily.Monospace, fontSize = 11.sp)
    }
}

@Composable
private fun WorkingDots() {
    Dots(MaterialTheme.colorScheme.primary, contentDescription = "Working")
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
            .clip(RoundedCornerShape(15.dp))
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(15.dp))
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
    val experimentMode by settings.experimentMode.collectAsStateWithLifecycle()
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Settings") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Text("Theme", fontWeight = FontWeight.SemiBold)
                MinimalThemePreset.entries.forEach { preset ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .background(
                                if (theme == preset) MaterialTheme.colorScheme.surfaceContainerHigh
                                else Color.Transparent,
                            )
                            .clickable { settings.setTheme(preset) }
                            .padding(9.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Box(
                            Modifier
                                .size(24.dp)
                                .clip(CircleShape)
                                .background(preset.preview)
                                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, CircleShape),
                        )
                        Text(preset.label, modifier = Modifier.weight(1f))
                        if (theme == preset) Text("✓", color = MaterialTheme.colorScheme.primary)
                    }
                }
                HorizontalDivider()
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Experiment mode", fontWeight = FontWeight.SemiBold)
                        Text(
                            "Use synchronized native chat instead of the direct agent terminal.",
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    Switch(
                        checked = experimentMode,
                        onCheckedChange = settings::setExperimentMode,
                    )
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
