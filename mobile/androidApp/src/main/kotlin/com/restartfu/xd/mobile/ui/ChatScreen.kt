package com.restartfu.xd.mobile.ui

import android.app.Activity
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.platform.LocalUriHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.graphics.Color
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TextField
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Button
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.OpenedQueue
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.model.AskBlock
import com.restartfu.xd.model.LinkedPullRequest
import com.restartfu.xd.model.AssistantSection
import com.restartfu.xd.model.AssistantSectionKind
import com.restartfu.xd.model.AssistantSections
import com.restartfu.xd.model.SubagentState
import com.restartfu.xd.model.AgentRelay
import com.restartfu.xd.model.ToolGrouping
import com.restartfu.xd.model.ToolText
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.ImageReference
import com.restartfu.xd.model.MessagePart
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.model.TranscriptRow
import com.restartfu.xd.model.TodoStatus
import com.restartfu.xd.model.DisplayLabels
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.syntax.CodeBlocks
import com.restartfu.xd.voice.SpeakTagVisibility

/** The desktop's conversation, diff, files and terminal panes, as tabs. */
internal enum class Pane(val label: String) {
    CHAT("Chat"),
    DIFF("Changes"),
    FILES("Files"),
    TERMINAL("Terminal"),
}

internal class WorkspacePaneModel : ViewModel() {
    var pane by mutableStateOf(Pane.CHAT)
    lateinit var chat: com.restartfu.xd.store.ChatSession
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ChatScreen(
    model: ChatViewModel,
    goBack: () -> Unit,
    sessionTitle: String = "",
    projectName: String = "",
    /** The session's checked-out branch, as the tree reports it. */
    branch: String? = null,
    /** Any chat or shell in the session is busy; they share one checkout. */
    sessionWorking: Boolean = false,
    /** Background agents and shells this chat's agent is still running, from the tree. */
    backgroundTasks: Int = 0,
    workspaceId: String = model.state.value.chatId,
    terminalChatId: String = model.state.value.chatId,
    workspaceOwner: ViewModelStoreOwner? = null,
    productHeader: @Composable () -> Unit = {},
    /**
     * The tablet's chat tabs, given the open chat's agent tabs to show after
     * them and what opens its Pull requests list, null where the host cannot
     * link them.
     */
    sessionTabs: @Composable (AgentTabs, (() -> Unit)?) -> Unit = { _, _ -> },
    /** The session's chats; given on phones, where one header bar holds them. */
    sessionChats: SessionChats? = null,
    /**
     * The chat's linked pull requests from the tree, on a host that keeps
     * them; null on an older one, which shows the branch's pull request alone.
     */
    linkedPullRequests: List<LinkedPullRequest>? = null,
    /** Chats this chat's agent started, as the tree has them now, by id. */
    agentChats: Map<String, com.restartfu.xd.model.ChatSummary> = emptyMap(),
    /** The chat whose agent started this one, if one did. */
    startedBy: com.restartfu.xd.model.ChatSummary? = null,
    /** Opens another of this host's chats by id. */
    openChat: (String) -> Unit = {},
) {
    val state by model.state.collectAsStateWithLifecycle()
    val sending by model.sending.collectAsStateWithLifecycle()
    val cancelling by model.cancelling.collectAsStateWithLifecycle()
    val steering by model.steering.collectAsStateWithLifecycle()
    val subagentActions by model.subagentActions.collectAsStateWithLifecycle()
    val agentTabKeys by model.agentTabs.collectAsStateWithLifecycle()
    val viewingAgent by model.viewingAgent.collectAsStateWithLifecycle()
    val agentDrafts by model.agentDrafts.collectAsStateWithLifecycle()
    val messagingAgent by model.messagingAgent.collectAsStateWithLifecycle()
    val composer by model.draft.collectAsStateWithLifecycle()
    val catalog by model.catalog.collectAsStateWithLifecycle()

    // The desktop shows these beside the conversation. A phone has room for
    // one at a time, so they are tabs over the same chat.
    val workspacePane: WorkspacePaneModel = viewModel(
        key = "workspace-pane-$workspaceId",
        viewModelStoreOwner = workspaceOwner ?: checkNotNull(androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner.current),
    )
    workspacePane.chat = model.session
    var pane by workspacePane::pane
    // On a phone the workspace views replace the chat under one header, so
    // Back returns to the chat before it leaves the session.
    androidx.activity.compose.BackHandler(enabled = sessionChats != null && pane != Pane.CHAT) { pane = Pane.CHAT }
    // An agent tab sits over the chat it belongs to, so Back returns to the chat.
    androidx.activity.compose.BackHandler(enabled = pane == Pane.CHAT && viewingAgent != null) { model.showChat() }
    var choosingModel by rememberSaveable(state.chatId) { mutableStateOf(false) }
    var choosingOptions by rememberSaveable(state.chatId) { mutableStateOf(false) }

    if (choosingOptions) {
        ChatOptionsDialog(model, state) { choosingOptions = false }
    }
    if (choosingModel) {
        ModelPicker(
            model = model,
            currentBackend = state.backend,
            currentModel = state.model,
            currentEffort = state.effort,
            onDismiss = { choosingModel = false },
        )
    }
    val pullRequest by model.pullRequest.collectAsStateWithLifecycle()
    val openLink = rememberLinkOpener()
    var showingPullRequests by rememberSaveable(state.chatId) { mutableStateOf(false) }
    val showPullRequests: (() -> Unit)? = if (linkedPullRequests != null) {
        { showingPullRequests = true }
    } else {
        null
    }
    if (showingPullRequests && linkedPullRequests != null) {
        val links by model.pullRequestLinks.collectAsStateWithLifecycle()
        val close = {
            showingPullRequests = false
            model.closePullRequests()
        }
        PullRequestsDialog(
            pullRequests = linkedPullRequests,
            draft = links.draft,
            onDraft = model::editPullRequestDraft,
            linking = links.linking,
            removing = links.removing,
            error = links.error,
            open = openLink,
            link = model::linkPullRequests,
            remove = model::unlinkPullRequest,
            onDismiss = close,
        )
    }
    // A finished turn may have pushed a branch or opened its pull request.
    // A sibling chat's turn shares the checkout, so the whole session counts,
    // and so does a branch switched under an idle session.
    val busy = state.working || sessionWorking
    // And so does a pull request another device made: the host reports it.
    val gitChanges by model.gitChanges.collectAsStateWithLifecycle()
    LaunchedEffect(state.chatId, busy, branch, gitChanges) {
        // While a turn runs the branch may still move, so only forget a stale link.
        model.refreshPullRequest(branch, fetch = !busy)
    }
    val listState = rememberLazyListState()
    val items = state.visibleItems

    // A run of ordinary tool calls collapses to one line; inline diffs keep
    // their own file-change row, and a lone call shows its command.
    val rows = remember(items) { ToolGrouping.rows(items) }
    val answered = remember(items) { AskBlock.answered(items) }
    // The chat's agents by record key: the host rewrites one record per agent,
    // so each key is one row holding the agent's newest snapshot.
    val agentRuns = remember(rows) {
        rows.filterIsInstance<TranscriptRow.Subagent>()
            .mapNotNull { row -> row.run.key?.let { it to row.run } }
            .toMap()
    }
    val runningAgents = remember(rows) {
        rows.filterIsInstance<TranscriptRow.Subagent>()
            .map { it.run }
            // An agent chat opens as a chat of its own, not a tab here.
            .filter { it.state == SubagentState.RUNNING && it.chat == null }
    }
    val agents = AgentTabs(
        tabs = agentTabKeys.map { key ->
            val run = agentRuns[key]
            AgentTab(
                key = key,
                title = run?.let(::agentTitle) ?: "Agent",
                provider = run?.provider.orEmpty(),
                running = run?.state == SubagentState.RUNNING,
            )
        },
        viewing = viewingAgent,
        open = { key ->
            pane = Pane.CHAT
            model.openAgent(key)
        },
        close = model::closeAgent,
        showChat = model::showChat,
    )
    val shownAgent = viewingAgent?.let { agentRuns[it] }
    var trayOpen by rememberSaveable(state.chatId) { mutableStateOf(false) }
    val leadingItemCount =
        (if (state.hasOlderMessages) 1 else 0) + (if (state.error != null) 1 else 0)
    // The working row sits below the last message, so it is what "the bottom"
    // means while a turn runs.
    // Between turns, the same place says what the agent still runs in the background.
    val waitingOn = if (state.working) 0 else backgroundTasks
    val trailingItemCount = if (state.working || waitingOn > 0) 1 else 0
    val lastTranscriptIndex = leadingItemCount + rows.lastIndex + trailingItemCount
    // Whether new output keeps the end in view. Only the reader's own drags
    // and flings change it (scrolling from code is not nested scrolling), so
    // a follow still animating when the next words arrive keeps following,
    // and a reader who scrolled back inside a long reply is left there.
    // Saved, so a rotation does not pull a reader who scrolled away back down.
    val following = rememberSaveable(state.chatId) { mutableStateOf(true) }
    val followReader = remember(listState, following) {
        object : NestedScrollConnection {
            override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
                following.value = !listState.canScrollForward
                return Offset.Zero
            }
        }
    }

    // Going into a chat lands on the newest message, not the oldest. This
    // cannot be inferred from the list state: by the time an effect runs the
    // items are already laid out, so "nothing measured yet" reads as a full
    // list and the jump never happens. Track whether this visit has landed.
    // Kept on the chat's model rather than in saved state: each chat's saved
    // state outlives leaving it, which would bring a reader back to wherever
    // they last were. A rotation keeps the flag, so it does not re-land and
    // throw away the scroll position rememberLazyListState just restored.
    val activity = LocalContext.current as? Activity
    DisposableEffect(model) {
        onDispose {
            if (activity?.isChangingConfigurations != true) model.landed = false
        }
    }

    LaunchedEffect(state.chatId, items.isNotEmpty()) {
        if (!model.landed && items.isNotEmpty()) {
            // Jumped, not animated: scrolling through a loaded history on open
            // is motion the reader did not ask for.
            listState.scrollToEnd(lastTranscriptIndex)
            following.value = true
            model.landed = true
        }
    }

    LaunchedEffect(items.size, items.lastOrNull()?.text, leadingItemCount, trailingItemCount) {
        if (model.landed && items.isNotEmpty() && following.value) {
            listState.scrollToEnd(lastTranscriptIndex, animated = true)
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            if (sessionChats != null) {
                Column {
                    productHeader()
                    val active = sessionChats.chats.firstOrNull { it.id == sessionChats.selectedId }
                    val chatTitle = active?.title?.ifBlank { null } ?: state.title.ifEmpty { "New chat" }
                    // An open agent names itself, under the chat it belongs to.
                    val agentTab = agents.tabs.firstOrNull { it.key == viewingAgent }.takeIf { pane == Pane.CHAT }
                    CompactChatBar(
                        title = agentTab?.title ?: chatTitle,
                        subtitle = if (agentTab != null) {
                            chatTitle
                        } else {
                            listOf(sessionTitle, projectName).filter { it.isNotBlank() }.joinToString(" · ")
                        },
                        backend = (if (agentTab != null) agentTab.provider.lowercase() else state.backend).ifBlank { null },
                        pullRequest = pullRequest,
                        openPullRequest = openLink,
                        pane = pane,
                        onPane = { pane = it },
                        // Back steps out of a workspace view or an agent to the chat first, as system Back does.
                        onBack = {
                            when {
                                pane != Pane.CHAT -> pane = Pane.CHAT
                                viewingAgent != null -> model.showChat()
                                else -> goBack()
                            }
                        },
                        onChatSettings = { choosingOptions = true },
                        chats = sessionChats,
                        agents = agents,
                        linkedPullRequests = linkedPullRequests,
                        showPullRequests = showPullRequests,
                    )
                }
            } else Column {
                productHeader()
                TopAppBar(
                    title = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column {
                                Text(
                                    sessionTitle.ifBlank { state.title.ifEmpty { "Session" } },
                                    style = MaterialTheme.typography.titleMedium,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    if (projectName.isNotBlank()) Text(
                                        projectName,
                                        modifier = Modifier.weight(1f, fill = false),
                                        style = MaterialTheme.typography.labelMedium,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                    if (linkedPullRequests != null) {
                                        // Every pull request linked to the chat, as the desktop's header lists them.
                                        LinkedPullRequestChips(
                                            pullRequests = linkedPullRequests,
                                            limit = PullRequestChipLimit.TABLET,
                                            style = MaterialTheme.typography.labelMedium,
                                            open = openLink,
                                            showAll = { showingPullRequests = true },
                                        )
                                    } else {
                                        // The branch's pull request, as the desktop's session header shows it.
                                        pullRequest?.let { url ->
                                            Text(
                                                pullRequestLabel(url),
                                                modifier = Modifier.clickable(role = Role.Button) { openLink(url) },
                                                style = MaterialTheme.typography.labelMedium,
                                                color = LocalXdSignals.current.link,
                                                textDecoration = TextDecoration.Underline,
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    },
                    navigationIcon = {
                        IconButton(onClick = goBack) {
                            Icon(painterResource(R.drawable.ic_back), contentDescription = "Back")
                        }
                    },
                    actions = {
                        if (pane == Pane.CHAT) {
                            TextButton(
                                onClick = { choosingModel = true },
                                colors = ButtonDefaults.textButtonColors(
                                    contentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                                ),
                            ) {
                                Text(
                                    DisplayLabels.model(state.model, state.backend, catalog),
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.widthIn(max = 110.dp),
                                )
                            }
                            IconButton(onClick = { choosingOptions = true }) {
                                Icon(
                                    painter = painterResource(R.drawable.ic_settings),
                                    contentDescription = "Chat settings",
                                )
                            }
                        }
                    },
                )
                sessionTabs(agents, showPullRequests)
                TabRow(
                    selectedTabIndex = pane.ordinal,
                    containerColor = MaterialTheme.colorScheme.surface,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                    divider = { HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant) },
                ) {
                    Pane.entries.forEach { candidate ->
                        Tab(
                            selected = pane == candidate,
                            onClick = { pane = candidate },
                            text = { Text(candidate.label) },
                            selectedContentColor = MaterialTheme.colorScheme.onSurface,
                            unselectedContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        },
        bottomBar = {
            if (pane == Pane.CHAT && viewingAgent != null) {
                val key = viewingAgent
                AgentComposer(
                    run = shownAgent,
                    draft = key?.let { agentDrafts[it] }.orEmpty(),
                    onDraft = { text -> key?.let { model.updateAgentDraft(it, text) } },
                    sending = messagingAgent,
                    onSend = { shownAgent?.let(model::messageAgent) },
                    compact = sessionChats != null,
                )
            } else if (pane == Pane.CHAT) {
                Column {
                    AgentsTray(
                        running = runningAgents,
                        open = trayOpen,
                        onToggle = { trayOpen = !trayOpen },
                        onOpenAgent = agents.open,
                        compact = sessionChats != null,
                        // The chat's agent chats as the tree has them now: their
                        // rows may be far up the chat, or not loaded at all.
                        workers = agentChats.values.filter { it.working || com.restartfu.xd.model.AgentsTray.needsYou(it) },
                        onOpenChat = openChat,
                    )
                    state.interactions.firstOrNull()?.let { request -> InteractionCard(request, model) }
                Composer(
                    state = state,
                    composer = composer,
                    sending = sending,
                    cancelling = cancelling,
                    steering = steering,
                    model = model,
                    compact = sessionChats != null,
                    onModel = { choosingModel = true },
                )
                }
            }
        },
    ) { padding ->
        if (pane != Pane.CHAT) {
            Box(Modifier.padding(padding)) {
                when (pane) {
                    Pane.DIFF -> DiffPaneContent(
                        viewModel(
                            key = "diff-$workspaceId",
                            viewModelStoreOwner = workspaceOwner ?: checkNotNull(androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner.current),
                            factory = DiffViewModel.Factory { workspacePane.chat },
                        ),
                    )
                    Pane.FILES -> FilesPaneContent(
                        viewModel(
                            key = "files-$workspaceId",
                            viewModelStoreOwner = workspaceOwner ?: checkNotNull(androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner.current),
                            factory = FilesViewModel.Factory { workspacePane.chat },
                        ),
                    )
                    Pane.TERMINAL -> {
                        val terminal: TerminalViewModel = viewModel(
                            key = "terminal-$workspaceId",
                            viewModelStoreOwner = workspaceOwner ?: checkNotNull(androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner.current),
                            factory = TerminalViewModel.ShellFactory(model.client, terminalChatId),
                        )
                        TerminalPaneContent(terminal)
                    }
                    else -> Unit
                }
            }
            return@Scaffold
        }
        if (viewingAgent != null) {
            val address = shownAgent?.messageAddress
            val sent = remember(items, address) {
                if (address == null) {
                    emptyList()
                } else {
                    items.filter { it.kind == TranscriptKind.USER }
                        .mapNotNull { AgentRelay.parse(it.text) }
                        .filter { it.address == address }
                }
            }
            val queued = remember(state.queue, address) {
                if (address == null) emptyList() else state.queue.mapNotNull(AgentRelay::parse).filter { it.address == address }
            }
            val pending = viewingAgent?.let { subagentActions[it] }
            shownAgent?.let { SettleSubagentAction(it, pending, model) }
            androidx.compose.runtime.key(viewingAgent) {
                AgentView(
                    run = shownAgent,
                    sent = sent,
                    queued = queued,
                    pendingAction = pending,
                    onAction = model::controlSubagent,
                    modifier = Modifier.padding(padding),
                )
            }
            return@Scaffold
        }
        if (state.transcriptLoaded && !state.loading && !state.hasOlderMessages &&
            items.isEmpty() && state.error == null && !state.working
        ) {
            Box(
                Modifier.fillMaxSize().padding(padding).padding(horizontal = 28.dp),
                contentAlignment = Alignment.Center,
            ) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    modifier = Modifier.widthIn(max = 480.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Icon(painterResource(R.drawable.ic_folder), null, modifier = Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text(projectName.ifBlank { sessionTitle.ifBlank { "Workspace" } }, style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Text("What are we working on?", style = MaterialTheme.typography.headlineSmall,
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                    Text("A new conversation in the same workspace.", style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                    Text("History, drafts, and agent settings stay in this chat.", style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                }
            }
            return@Scaffold
        }
        LazyColumn(
            state = listState,
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .nestedScroll(followReader),
            contentPadding = PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (startedBy != null) {
                item(key = "started-by") { StartedByPill(startedBy) { openChat(startedBy.id) } }
            }
            if (state.hasOlderMessages) {
                item(key = "load-older") {
                    Box(
                        modifier = Modifier.fillMaxWidth(),
                        contentAlignment = Alignment.Center,
                    ) {
                        TextButton(
                            onClick = model::loadOlder,
                            enabled = !state.loadingOlder,
                        ) {
                            Text(if (state.loadingOlder) "Loading…" else "Load older")
                        }
                    }
                }
            }
            if (state.loading && items.isEmpty()) {
                item { CircularProgressIndicator() }
            }
            state.error?.let { error ->
                item { Text(error, color = MaterialTheme.colorScheme.error) }
            }
            items(rows, key = ::rowKey) { row ->
                when (row) {
                    is TranscriptRow.Single -> {
                        // xd's word that one of this chat's agents ended a turn.
                        val report = row.item.takeIf { it.kind == com.restartfu.xd.model.TranscriptKind.USER }
                            ?.let { com.restartfu.xd.model.AgentReport.parse(it.text) }
                        if (report != null) {
                            AgentReportLine(report, agentChats[report.chatId]) { openChat(report.chatId) }
                        } else {
                            TranscriptRow(row.item, model, answered[row.item.id])
                        }
                    }
                    is TranscriptRow.Tools -> ToolGroupRow(row)
                    is TranscriptRow.AccountSwitch -> AccountSwitchNotice(row.sentence)
                    is TranscriptRow.Pipeline -> PipelineCard(row.run, model.client)
                    is TranscriptRow.Subagent -> if (row.run.chat != null) {
                        // An agent started with start_agent is a chat of its own.
                        val chat = row.run.chat.orEmpty()
                        AgentChatRow(row.run, agentChats[chat]) { openChat(chat) }
                    } else {
                        val key = row.run.key
                        val pending = key?.let { subagentActions[it] }
                        SettleSubagentAction(row.run, pending, model)
                        SubagentCard(
                            row.run,
                            pending,
                            model::controlSubagent,
                            onOpen = key?.let { { agents.open(it) } },
                        )
                    }
                }
            }
            if (state.working) {
                item(key = "working") { WorkingRow(state.startedAtMillis) }
            } else if (waitingOn > 0) {
                item(key = "background-tasks") {
                    androidx.compose.runtime.key(model.session) {
                        BackgroundTasksRow(
                            count = waitingOn,
                            visible = listState.layoutInfo.visibleItemsInfo.any { it.key == "background-tasks" },
                            load = { model.session.backgroundTaskOutput() },
                        )
                    }
                }
            }
        }
    }
}

/** Forgets an agent's pending Stop or Background once its record shows it took effect. */
@Composable
private fun SettleSubagentAction(run: com.restartfu.xd.model.SubagentRun, pending: String?, model: ChatViewModel) {
    val key = run.key ?: return
    if (pending == null) return
    val settled = run.state != SubagentState.RUNNING ||
        (pending == "Moving to background" && run.background)
    LaunchedEffect(settled) { if (settled) model.settleSubagentAction(key) }
}

/**
 * The composer, with the desktop's one round button: an arrow while the next
 * message is yours to send, a stop while the turn is the assistant's to finish.
 *
 * One turn per chat is a host rule, so a message written while one runs is
 * queued rather than sent. That happens from the keyboard's own send key, which
 * is what Enter does on the desktop for the same reason: the button is a stop
 * by then.
 */
@Composable
private fun Composer(
    state: com.restartfu.xd.model.ChatState,
    composer: String,
    sending: Boolean,
    cancelling: Boolean,
    steering: Boolean,
    model: ChatViewModel,
    compact: Boolean = false,
    onModel: () -> Unit = {},
) {
    val context = LocalContext.current
    val attachments by model.attachments.collectAsStateWithLifecycle()
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val attachmentError by model.attachmentError.collectAsStateWithLifecycle()
    // The queue as it was when a message was opened: actions apply to that
    // queue only, and the dialog closes if another device changes it.
    // In the view model, so an open queued message survives rotating the phone.
    val opened by model.openQueued.collectAsStateWithLifecycle()
    val editing = opened?.let { Triple(it.index, it.text, it.queue) }
    val openDraft = opened?.draft
    val queueNotice by model.queueNotice.collectAsStateWithLifecycle()
    LaunchedEffect(state.queue, state.queueRevision) {
        val opened = editing ?: return@LaunchedEffect
        val revision = opened.third.revision
        if (state.queue != opened.third.messages || (revision != null && state.queueRevision != revision)) {
            model.closeQueuedMessage()
            model.queueChanged()
        }
    }

    // Another queue action still out would make these do nothing, so they wait.
    val queueBusy by model.queueBusy.collectAsStateWithLifecycle()
    editing?.let { (index, original, opened) ->
        // Queued messages have no identity but their place, and texts can
        // repeat, so changing one needs a host that keeps a queue revision.
        val identifiable = opened.revision != null
        QueuedMessageDialog(
            draft = openDraft ?: original,
            onDraft = model::editQueuedDraft,
            original = original,
            busy = queueBusy || steering,
            working = state.working,
            canMoveUp = index > 0 && identifiable,
            canMoveDown = index < state.queue.lastIndex && identifiable,
            canDrop = identifiable,
            canEdit = identifiable,
            onMove = { up ->
                model.closeQueuedMessage()
                model.moveQueued(index, original, up, opened)
            },
            onDismiss = { model.closeQueuedMessage() },
            onSave = { text ->
                model.closeQueuedMessage()
                if (text != original) model.editQueued(index, original, text, opened)
            },
            onSteer = { text ->
                model.closeQueuedMessage()
                model.steerQueued(index, original, text, opened)
            },
            onDrop = {
                model.closeQueuedMessage()
                model.dropQueued(index, original, opened)
            },
        )
    }
    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.PickMultipleVisualMedia(Limits.MAX_IMAGES),
    ) { uris -> model.attach(context, uris) }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .imePadding()
            .padding(horizontal = if (compact) 8.dp else 12.dp, vertical = if (compact) 6.dp else 10.dp),
    ) {
        attachmentError?.let { message ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    message,
                    modifier = Modifier.weight(1f),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
                TextButton(onClick = model::clearAttachmentError) { Text("Dismiss") }
            }
        }
        if (attachments.isNotEmpty()) {
            Row(
                modifier = Modifier
                    .horizontalScroll(rememberScrollState())
                    .padding(bottom = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                attachments.forEachIndexed { index, attachment ->
                    Box {
                        Image(
                            bitmap = attachment.thumbnail,
                            contentDescription = "Attached image",
                            modifier = Modifier
                                .size(64.dp)
                                .clip(MaterialTheme.shapes.small),
                            contentScale = ContentScale.Crop,
                        )
                        IconButton(
                            onClick = { model.removeAttachment(index) },
                            modifier = Modifier.align(Alignment.TopEnd).size(48.dp),
                        ) {
                            Icon(painterResource(R.drawable.ic_close), "Remove attached image", modifier = Modifier.size(20.dp))
                        }
                    }
                }
            }
        }
        remember(state) { AskBlock.pending(state) }?.let { ask ->
            AskButtons(ask, onAnswer = model::answer, enabled = !sending)
        }
        if (state.todos.isNotEmpty()) {
            val completed = state.todos.count { it.status == TodoStatus.COMPLETED }
            Surface(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 160.dp),
                shape = RoundedCornerShape(14.dp),
                color = MaterialTheme.colorScheme.surfaceContainer,
                border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            ) {
                Column(
                    modifier = Modifier
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Text(
                        "Tasks  $completed/${state.todos.size}",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    state.todos.forEach { todo ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(
                                when (todo.status) {
                                    TodoStatus.PENDING -> "○"
                                    TodoStatus.IN_PROGRESS -> "●"
                                    TodoStatus.COMPLETED -> "✓"
                                },
                                color = when (todo.status) {
                                    TodoStatus.PENDING -> MaterialTheme.colorScheme.onSurfaceVariant
                                    TodoStatus.IN_PROGRESS -> MaterialTheme.colorScheme.primary
                                    TodoStatus.COMPLETED -> MaterialTheme.colorScheme.tertiary
                                },
                            )
                            Text(
                                todo.text,
                                style = MaterialTheme.typography.bodySmall,
                                color = if (todo.status == TodoStatus.COMPLETED) {
                                    MaterialTheme.colorScheme.onSurfaceVariant
                                } else {
                                    MaterialTheme.colorScheme.onSurface
                                },
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.height(8.dp))
        }
        if (state.shortcuts.isNotEmpty()) {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                state.shortcuts.forEach { prompt ->
                    AssistChip(
                        onClick = { model.shortcut(prompt) },
                        enabled = !sending,
                        colors = AssistChipDefaults.assistChipColors(
                            containerColor = MaterialTheme.colorScheme.primaryContainer,
                            labelColor = MaterialTheme.colorScheme.onPrimaryContainer,
                            disabledContainerColor = MaterialTheme.colorScheme.surfaceVariant,
                            disabledLabelColor = MaterialTheme.colorScheme.onSurfaceVariant,
                        ),
                        label = {
                            Text(
                                prompt,
                                modifier = Modifier.widthIn(max = 240.dp),
                                maxLines = 2,
                                overflow = TextOverflow.Ellipsis,
                            )
                        },
                    )
                }
            }
        }
        queueNotice?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        }
        if (state.queue.isNotEmpty()) {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                state.queue.forEachIndexed { index, queued ->
                    AssistChip(
                        // Tapping opens the controls rather than dropping:
                        // discarding a queued message on a stray tap, with no
                        // undo, is not a good trade for one less tap.
                        onClick = { model.openQueuedMessage(index) },
                        label = {
                            Text(
                                AgentRelay.parse(queued)?.let { "${agentRelayLabel(it.description)}: ${it.text}" } ?: queued,
                                modifier = Modifier.widthIn(max = 220.dp),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        },
                    )
                }
            }
        }
        Spacer(Modifier.height(6.dp))
        if (compact) {
            CompactComposerRow(state, composer, sending, cancelling, steering, model, attachments.size, onModel) {
                picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            }
            return@Column
        }
        // One rounded box, the way the desktop draws it: the message on top,
        // attach and send underneath.
        Surface(
            shape = RoundedCornerShape(20.dp),
            color = MaterialTheme.colorScheme.surfaceContainer,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        ) {
            Column(Modifier.padding(start = 4.dp, end = 8.dp, top = 2.dp, bottom = 6.dp)) {
                TextField(
                    value = composer,
                    onValueChange = model::updateDraft,
                    modifier = Modifier.fillMaxWidth(),
                    placeholder = { Text("Ask anything, or describe a task…", color = LocalXdSignals.current.faint) },
                    minLines = 1,
                    maxLines = 6,
                    colors = TextFieldDefaults.colors(
                        focusedContainerColor = Color.Transparent,
                        unfocusedContainerColor = Color.Transparent,
                        disabledContainerColor = Color.Transparent,
                        focusedIndicatorColor = Color.Transparent,
                        unfocusedIndicatorColor = Color.Transparent,
                        disabledIndicatorColor = Color.Transparent,
                    ),
                    // The desktop queues with Enter while a turn runs, since its
                    // button is the stop. The keyboard's own action key is where
                    // that lives here.
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(
                        onSend = { if (state.working) model.enqueue() else model.send() },
                    ),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    // An icon button rather than a text button: a lone glyph in a
                    // label-sized run of text is a small thing to hit, and this
                    // one gets a square touch target.
                    IconButton(
                        onClick = {
                            picker.launch(
                                PickVisualMediaRequest(
                                    ActivityResultContracts.PickVisualMedia.ImageOnly,
                                ),
                            )
                        },
                        enabled = attachments.size < Limits.MAX_IMAGES,
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_add),
                            contentDescription = "Attach images",
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.size(20.dp),
                        )
                    }
                    Text(
                        DisplayLabels.backend(state.backend, catalog),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.weight(1f))
                    SendButton(
                        state = state,
                        empty = composer.isBlank() && attachments.isEmpty(),
                        sending = sending,
                        cancelling = cancelling,
                        steering = steering,
                        model = model,
                        size = 48.dp,
                    )
                }
            }
        }
    }
}

/**
 * The composer as one row: attach, the message, the agent and model, send.
 *
 * The agent and model sit where they are chosen for the next message rather
 * than in the header, the way the desktop composer carries them. The message
 * grows upward to six lines; the controls stay on its last line.
 */
@Composable
private fun CompactComposerRow(
    state: com.restartfu.xd.model.ChatState,
    composer: String,
    sending: Boolean,
    cancelling: Boolean,
    steering: Boolean,
    model: ChatViewModel,
    attached: Int,
    onModel: () -> Unit,
    onAttach: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val faint = LocalXdSignals.current.faint
    Surface(
        shape = RoundedCornerShape(24.dp),
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Row(
            modifier = Modifier.padding(4.dp),
            verticalAlignment = Alignment.Bottom,
        ) {
            IconButton(onClick = onAttach, enabled = attached < Limits.MAX_IMAGES, modifier = Modifier.size(40.dp)) {
                Icon(
                    painterResource(R.drawable.ic_add),
                    contentDescription = "Attach images",
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(20.dp),
                )
            }
            androidx.compose.foundation.text.BasicTextField(
                value = composer,
                onValueChange = model::updateDraft,
                modifier = Modifier.weight(1f).heightIn(min = 40.dp),
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                cursorBrush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.onSurface),
                maxLines = 6,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(
                    onSend = { if (state.working) model.enqueue() else model.send() },
                ),
                decorationBox = { field ->
                    Box(Modifier.padding(start = 4.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), contentAlignment = Alignment.CenterStart) {
                        if (composer.isEmpty()) {
                            Text(
                                "Ask anything…",
                                style = MaterialTheme.typography.bodyLarge,
                                color = faint,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        field()
                    }
                },
            )
            // Drawn at 32dp in the 40dp control line. The platform still
            // extends its touch area to 48dp around it.
            Box(Modifier.height(40.dp), contentAlignment = Alignment.Center) {
            CompositionLocalProvider(androidx.compose.material3.LocalMinimumInteractiveComponentSize provides 0.dp) {
            Surface(
                onClick = onModel,
                shape = RoundedCornerShape(16.dp),
                color = Color.Transparent,
                modifier = Modifier.padding(horizontal = 2.dp),
            ) {
                Row(
                    modifier = Modifier.height(32.dp).padding(horizontal = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    BackendIcon(state.backend, size = 14.dp)
                    Text(
                        DisplayLabels.model(state.model, state.backend, catalog),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.widthIn(max = 96.dp),
                    )
                }
            }
            }
            }
            SendButton(
                state = state,
                empty = composer.isBlank() && attached == 0,
                sending = sending,
                cancelling = cancelling,
                steering = steering,
                model = model,
                size = 40.dp,
            )
        }
    }
}

/**
 * One round button that sends and then stops, as on the desktop: an arrow in
 * the accent while it is yours to send, a neutral stop while the turn is the
 * assistant's to finish. With nothing typed after a turn was cut short, the
 * same button continues it, as the desktop's play button does.
 */
@Composable
private fun SendButton(
    state: com.restartfu.xd.model.ChatState,
    empty: Boolean,
    sending: Boolean,
    cancelling: Boolean,
    steering: Boolean,
    model: ChatViewModel,
    size: androidx.compose.ui.unit.Dp,
) {
    val resuming = !state.working && state.resumable && empty
    FilledIconButton(
        onClick = {
            when {
                state.working -> model.cancel()
                resuming -> model.resumeTurn()
                else -> model.send()
            }
        },
        enabled = when {
            state.working -> !cancelling && !steering
            resuming -> !sending
            // The host accepts text, images, or both.
            else -> !sending && !empty
        },
        modifier = Modifier.size(size),
        colors = if (state.working) {
            IconButtonDefaults.filledIconButtonColors(
                containerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                contentColor = MaterialTheme.colorScheme.onSurface,
            )
        } else {
            IconButtonDefaults.filledIconButtonColors(
                disabledContainerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                disabledContentColor = LocalXdSignals.current.faint,
            )
        },
    ) {
        Icon(
            painter = painterResource(
                when {
                    state.working -> R.drawable.ic_stop
                    resuming -> R.drawable.ic_play
                    else -> R.drawable.ic_send
                },
            ),
            contentDescription = when {
                cancelling -> "Stopping"
                state.working -> "Stop"
                sending -> "Sending"
                resuming -> "Continue where it left off"
                else -> "Send"
            },
            modifier = Modifier.size(if (state.working || resuming) size / 3 else size * 5 / 12),
        )
    }
}

/**
 * Edit, steer or drop one queued message.
 *
 * Steering is only offered while a turn is running, because that is the only
 * time it means anything: the host redirects the turn to the message so the
 * agent takes it up instead of finishing first.
 */
@Composable
private fun QueuedMessageDialog(
    original: String,
    /** The edit in progress, held by the view model with the open message. */
    draft: String,
    onDraft: (String) -> Unit,
    /** A queue action is still out; actions wait for it rather than being dropped. */
    busy: Boolean,
    working: Boolean,
    canMoveUp: Boolean,
    canMoveDown: Boolean,
    canDrop: Boolean,
    canEdit: Boolean,
    onMove: (up: Boolean) -> Unit,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
    onSteer: (String) -> Unit,
    onDrop: () -> Unit,
) {
    val text = draft

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Queued message") },
        text = {
            Column {
                OutlinedTextField(
                    value = text,
                    onValueChange = onDraft,
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Message") },
                    minLines = 2,
                    maxLines = 6,
                )
                if (canMoveUp || canMoveDown) {
                    // Reordering moves the message as it was queued; an edit
                    // still in the field above is not sent with it.
                    Row {
                        TextButton(onClick = { onMove(true) }, enabled = !busy && canMoveUp && text == original) {
                            Text("Move earlier")
                        }
                        TextButton(onClick = { onMove(false) }, enabled = !busy && canMoveDown && text == original) {
                            Text("Move later")
                        }
                    }
                }
                if (!canEdit) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Update the host to change queued messages from here.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
                if (working) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Steering stops the running turn and starts this instead.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.outline,
                    )
                }
            }
        },
        confirmButton = {
            Row {
                if (working) {
                    TextButton(
                        onClick = { onSteer(text) },
                        enabled = !busy && canEdit && text.isNotBlank(),
                    ) { Text("Steer") }
                }
                TextButton(
                    onClick = { onSave(text) },
                    enabled = !busy && canEdit && text.isNotBlank(),
                ) { Text("Save") }
            }
        },
        dismissButton = {
            Row {
                TextButton(onClick = onDrop, enabled = !busy && canDrop) { Text("Drop") }
                TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}

/**
 * Message text as rendered Markdown.
 *
 * A user's own message and system text are shown verbatim: they were typed,
 * not authored as Markdown, and rendering them would eat characters the sender
 * meant literally.
 */
@Composable
private fun MessageBody(item: TranscriptItem, model: ChatViewModel) {
    val parts = remember(item.text, item.kind) {
        ImageReference.parts(item.text, markdown = item.kind == TranscriptKind.ASSISTANT)
    }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        parts.forEach { part ->
            when (part) {
                is MessagePart.Image -> MessageImage(model, part.path)
                is MessagePart.Prose -> when (item.kind) {
                    TranscriptKind.ASSISTANT -> AssistantProse(
                        text = part.text,
                        messageId = item.id,
                        live = item.live,
                    )
                    TranscriptKind.SYSTEM ->
                        Text(
                            part.text,
                            fontFamily = if (item.label == "Host") null else XdMonoFamily,
                            color = if (item.label == "Host") MaterialTheme.colorScheme.onSurfaceVariant
                                else androidx.compose.material3.LocalContentColor.current,
                        )
                    else -> Text(part.text)
                }
            }
        }
    }
}

/** Native long-press selection, scoped to one message at a time. */
@Composable
private fun SelectableMessageBody(item: TranscriptItem, model: ChatViewModel) {
    SelectionContainer { MessageBody(item, model) }
}

/**
 * Assistant prose with its client-side presentation blocks lifted out.
 *
 * `<ask>` becomes the existing question controls, `<analysis>` is an initially
 * closed disclosure, and `<summary>` is ordinary visible Markdown.
 */
@Composable
private fun AssistantProse(
    text: String,
    messageId: String,
    live: Boolean,
) {
    val ask = remember(text) { AskBlock.parse(text) }
    val source = ask?.remainder ?: text
    val visible = remember(source, live) {
        val projected = if (live) AssistantSections.stream(source) else source
        SpeakTagVisibility.render(projected, live)
    }
    val sections = remember(visible) { AssistantSections.parse(visible) }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        sections.forEachIndexed { index, section ->
            when (section.kind) {
                AssistantSectionKind.NORMAL ->
                    if (section.text.isNotBlank()) MarkdownText(section.text)
                AssistantSectionKind.ANALYSIS ->
                    AnalysisDisclosure(messageId, index, section)
            }
        }
        ask?.let { Text(it.ask.question, fontWeight = FontWeight.Bold) }
    }
}

@Composable
private fun AnalysisDisclosure(
    messageId: String,
    ordinal: Int,
    section: AssistantSection,
) {
    val key = "$messageId-analysis-$ordinal"
    var expanded by rememberSaveable(key) { mutableStateOf(false) }

    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { expanded = !expanded }
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "Analysis",
                    modifier = Modifier.weight(1f),
                    color = LocalXdSignals.current.faint,
                    style = MaterialTheme.typography.bodySmall,
                )
                Disclosure(expanded)
            }
            if (expanded && section.text.isNotBlank()) {
                MarkdownText(
                    section.text,
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                )
            }
        }
    }
}

/**
 * Shows the end of the item at [index]. Scrolling to an item alone puts its
 * top at the top of the list, so a last message taller than the screen would
 * show its start with the rest of it still below.
 */
private suspend fun LazyListState.scrollToEnd(index: Int, animated: Boolean = false) {
    // An item already in view is scrolled on from where it is: going to its
    // top first would run back through a tall reply before coming down again.
    if (layoutInfo.visibleItemsInfo.none { it.index == index }) {
        if (animated) animateScrollToItem(index) else scrollToItem(index)
    }
    while (canScrollForward) {
        val info = layoutInfo
        val last = info.visibleItemsInfo.lastOrNull() ?: break
        // What is left of the last item in view below the list's end.
        val left = last.offset + last.size - (info.viewportEndOffset - info.afterContentPadding)
        val step = (if (left > 0) left else info.viewportSize.height).coerceAtLeast(1).toFloat()
        val moved = if (animated) animateScrollBy(step) else scrollBy(step)
        if (moved <= 0f) break
    }
}

private fun rowKey(row: TranscriptRow): String = when (row) {
    is TranscriptRow.Single -> row.item.id
    // Keyed on the run's first call, which is stable as the run grows.
    is TranscriptRow.Tools -> "tools-${row.items.first().id}"
    is TranscriptRow.AccountSwitch -> "account-switch-${row.item.id}"
    is TranscriptRow.Pipeline -> "pipeline-${row.item.id}"
    is TranscriptRow.Subagent -> "subagent-${row.item.id}"
}

/**
 * A run of tool calls behind one line.
 *
 * Expanding shows each call's command. Their detail stays behind its own
 * toggle, so opening a run of ten does not unfold ten command outputs at once.
 */
@Composable
private fun ToolGroupRow(row: TranscriptRow.Tools) {
    var expanded by rememberSaveable(rowKey(row)) { mutableStateOf(false) }

    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        shape = RoundedCornerShape(14.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clickable { expanded = !expanded }
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    row.label,
                    modifier = Modifier.weight(1f),
                    color = LocalXdSignals.current.faint,
                    style = MaterialTheme.typography.bodySmall,
                )
                Disclosure(expanded)
            }
            if (expanded) {
                Spacer(Modifier.height(8.dp))
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    row.items.forEach { ToolRow(it, nested = true) }
                }
            }
        }
    }
}

/** The chevron at the end of anything that opens. */
@Composable
private fun Disclosure(expanded: Boolean) {
    Icon(
        painterResource(if (expanded) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
        contentDescription = if (expanded) "Collapse" else "Expand",
        tint = LocalXdSignals.current.faint,
        modifier = Modifier.size(14.dp),
    )
}

/**
 * A tool call or an inline diff, collapsed to one line.
 *
 * A phone has too little room to spend it on a patch or a wall of command
 * output nobody asked to see, so both start closed and open on tap. A tool row
 * with nothing but its summary gets no disclosure control at all.
 */
@Composable
private fun ToolRow(item: TranscriptItem, nested: Boolean = false) {
    val summary = remember(item.text) { ToolText.summary(item.text) }
    val detail = remember(item.text) { ToolText.detail(item.text) }
    val isPatch = remember(item.text) { ToolText.patch(item.text) != null }
    var expanded by rememberSaveable(item.id) { mutableStateOf(false) }

    Surface(
        color = if (nested) Color.Transparent else MaterialTheme.colorScheme.surfaceContainer,
        shape = RoundedCornerShape(if (nested) 8.dp else 14.dp),
        border = if (nested) null else BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .let { if (detail == null) it else it.clickable { expanded = !expanded } }
                .padding(horizontal = if (nested) 0.dp else 14.dp, vertical = if (nested) 5.dp else 10.dp),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    summary,
                    modifier = Modifier.weight(1f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = XdMonoFamily,
                )
                if (detail != null) Disclosure(expanded)
            }
            if (expanded && detail != null) {
                Spacer(Modifier.height(8.dp))
                if (isPatch) {
                    DiffText(detail)
                } else {
                    Text(
                        detail,
                        modifier = Modifier.horizontalScroll(rememberScrollState()),
                        fontFamily = XdMonoFamily,
                        style = MaterialTheme.typography.bodySmall,
                        softWrap = false,
                    )
                }
            }
        }
    }
}

/**
 * The chat moved to another of the assistant's accounts: quiet, like any
 * host notice, with no card, and the sentence selectable.
 */
@Composable
private fun AccountSwitchNotice(sentence: String) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(
            painterResource(com.restartfu.xd.mobile.R.drawable.ic_swap),
            contentDescription = "Account switched",
            tint = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(top = 2.dp).size(16.dp),
        )
        SelectionContainer(Modifier.weight(1f)) {
            Text(sentence, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * One message.
 *
 * Only the reader's own messages get a bubble, as on the desktop. A reply is
 * the page: it is most of what is on screen, it is the thing being read, and
 * carding it too would leave the two indistinguishable — a wall of identical
 * boxes where the only difference is which side is narrower.
 */
@Composable
private fun TranscriptRow(item: TranscriptItem, model: ChatViewModel, answering: String? = null) {
    if (item.kind == TranscriptKind.TOOL) {
        ToolRow(item)
        return
    }
    if (item.kind == TranscriptKind.SYSTEM && item.label == "Host") {
        Row(
            Modifier.fillMaxWidth().padding(vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Icon(
                painterResource(com.restartfu.xd.mobile.R.drawable.ic_alert),
                contentDescription = "Host notice",
                tint = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(top = 3.dp).size(16.dp),
            )
            Box(Modifier.weight(1f)) { SelectableMessageBody(item, model) }
        }
        return
    }
    if (item.kind != TranscriptKind.USER) {
        // A reply is the page itself, without a name over it; only system
        // text keeps its label, so it cannot be mistaken for the agent.
        Column(Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
            if (item.kind == TranscriptKind.SYSTEM) {
                item.label?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.labelSmall,
                        color = LocalXdSignals.current.faint,
                    )
                }
            }
            SelectableMessageBody(item, model)
        }
        return
    }
    // A message the reader sent one of the agents shows only what they wrote,
    // under the agent it went to; the lead's instructions around it are xd's.
    val relay = remember(item.text) { AgentRelay.parse(item.text) }
    val shown = if (relay != null) item.copy(text = relay.text) else item
    Box(
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        contentAlignment = Alignment.CenterEnd,
    ) {
        Column(Modifier.fillMaxWidth(0.86f), horizontalAlignment = Alignment.End) {
            // What the message answers, attached above it like a reply's quote.
            (answering?.let(::answeredQuestionLabel) ?: relay?.let { agentRelayLabel(it.description) })?.let { caption ->
                Row(
                    modifier = Modifier.padding(start = 4.dp, end = 4.dp, bottom = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(
                        painterResource(com.restartfu.xd.mobile.R.drawable.ic_reply),
                        contentDescription = null,
                        tint = LocalXdSignals.current.faint,
                        modifier = Modifier.padding(end = 5.dp).size(14.dp),
                    )
                    Text(
                        caption,
                        style = MaterialTheme.typography.labelMedium,
                        color = LocalXdSignals.current.faint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            Surface(
                // Tinted rather than the page colour: against plain reply text
                // the bubble has to carry the distinction on its own.
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                contentColor = MaterialTheme.colorScheme.onSurface,
                shape = RoundedCornerShape(20.dp),
            ) {
                Column(Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
                    SelectableMessageBody(shown, model)
                }
            }
        }
    }
}

/** The caption on a message the reader sent one of the agents, as on the desktop. */
internal fun agentRelayLabel(description: String): String = "To $description"

/** The caption on a message sent in reply to a question, as on the desktop. */
internal fun answeredQuestionLabel(question: String): String = "Answering \u201c${question.trim()}\u201d"

private fun approvalTitle(request: com.restartfu.xd.model.PendingInteraction): String =
    when (request.tool) {
        "Bash" -> "Run this command?"
        "Edit", "MultiEdit", "NotebookEdit" -> "Apply this edit?"
        "Write" -> "Write this file?"
        "WebFetch" -> "Fetch this page?"
        "WebSearch" -> "Search the web?"
        "ExitPlanMode" -> "Approve the plan?"
        null -> when (request.method) {
            "item/fileChange/requestApproval" -> "Apply these changes?"
            "item/commandExecution/requestApproval" -> "Run this command?"
            else -> "Approval needed"
        }
        else -> "Allow ${request.tool}?"
    }

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun InteractionCard(request: com.restartfu.xd.model.PendingInteraction, model: ChatViewModel) {
    var answers by remember(request.requestId) { mutableStateOf(emptyMap<String, String>()) }
    val waiting = LocalXdSignals.current.waiting
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            // Never taller than half the screen, so the answer stays in reach.
            .heightIn(max = (LocalConfiguration.current.screenHeightDp * 0.5f).dp),
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(1.dp, waiting.copy(alpha = 0.6f)),
    ) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (request.questions.isEmpty()) approvalTitle(request) else "Question",
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall,
                )
                StatusPill("Waiting on you", waiting)
            }
            Column(
                modifier = Modifier
                    .weight(1f, fill = false)
                    .verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (request.questions.isEmpty()) {
                    request.reason?.let {
                        Text(
                            it,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    Text(
                        request.detail,
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 140.dp)
                            .clip(RoundedCornerShape(10.dp))
                            .background(MaterialTheme.colorScheme.background)
                            .verticalScroll(rememberScrollState())
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                        fontFamily = XdMonoFamily,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    request.patch?.let { patch -> DiffText(patch) }
                } else {
                    request.questions.forEach { question ->
                        Text(question.text)
                        question.options.forEach { option ->
                            OutlinedButton(
                                onClick = { answers = answers + (question.id to option) },
                                modifier = Modifier.fillMaxWidth(),
                            ) { Text(option) }
                        }
                        OutlinedTextField(value = answers[question.id].orEmpty(), onValueChange = { answers = answers + (question.id to it) }, label = { Text("Answer") },
                            visualTransformation = if (question.secret) androidx.compose.ui.text.input.PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None)
                    }
                }
            }
            if (request.questions.isEmpty()) {
                FlowRow(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Button(onClick = { model.approve(request.requestId, "accept") }) { Text("Allow once") }
                    OutlinedButton(onClick = { model.approve(request.requestId, "acceptForSession") }) { Text("Allow for session") }
                    TextButton(onClick = { model.approve(request.requestId, "decline") }) {
                        Text("Deny", color = MaterialTheme.colorScheme.error)
                    }
                }
            } else {
                Button(onClick = { model.answer(request.requestId, answers) }, enabled = request.questions.all { !answers[it.id].isNullOrBlank() }) { Text("Submit answers") }
            }
        }
    }
}
