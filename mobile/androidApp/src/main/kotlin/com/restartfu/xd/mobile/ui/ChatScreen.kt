package com.restartfu.xd.mobile.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.runtime.SideEffect
import androidx.compose.foundation.layout.consumeWindowInsets
import android.app.Activity
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.Role
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.platform.LocalUriHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.ui.graphics.Color
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.material3.TextField
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.material3.FilterChip
import androidx.compose.material3.VerticalDivider
import androidx.compose.ui.platform.testTag
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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.snapshots.Snapshot
import com.restartfu.xd.model.ChatState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.ensureActive
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
import com.restartfu.xd.model.TranscriptRowCache
import com.restartfu.xd.model.key
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

    /** Whether a screen has given it the view saved with it; only a model made new takes one. */
    var restored = false
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
    /** Keeps the session's unsaved file edits if Android ends the app; null keeps them in memory only. */
    unsavedFiles: com.restartfu.xd.mobile.UnsavedFiles? = null,
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
    /**
     * The window is wide enough for two panes: Changes, Files and Terminal
     * open beside the chat instead of in its place.
     */
    sideBySide: Boolean = false,
) {
    // The chat as the host has it, words streaming in included. Read here it
    // would draw the whole screen again for every word, so the screen reads
    // [state], which leaves those words out, and only the reply's own row and
    // the follower read them.
    val streamingState = model.state.collectAsStateWithLifecycle()
    val state by remember(streamingState) { derivedStateOf { streamingState.value.withoutLiveText() } }
    val liveText = remember(streamingState) { { streamingState.value.liveSegment } }
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
    // The open view lives in the model, which a rotation keeps but the end of
    // the process does not: saved with the screen too, a model made new takes
    // the view that was open, rather than falling back to the chat.
    var savedPane by rememberSaveable(workspaceId) { mutableStateOf(workspacePane.pane) }
    // Only into a model made new: the session's chats share one, and a chat
    // tab shown again must not put back the view it had before.
    remember(workspacePane) {
        if (!workspacePane.restored) workspacePane.pane = savedPane
        workspacePane.restored = true
    }
    var pane by workspacePane::pane
    SideEffect { savedPane = pane }
    // Wherever a workspace view replaces the chat (a phone, or a tablet too
    // narrow for two panes, where the tab row switches them), Back returns to
    // the chat before it leaves the session. Beside the chat on a wide window,
    // Back closes the tool first, for the same reason.
    // Held as a gesture, the chat comes back in as it is dragged.
    val panes = rememberPageTransition(pane, "pane")
    PredictiveBack(panes, enabled = pane != Pane.CHAT, back = Pane.CHAT) { pane = Pane.CHAT }
    // The conversation is on screen: always beside a tool, otherwise while no tool has taken its place.
    val chatShown = sideBySide || pane == Pane.CHAT
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
    // A screen reader is told when the turn ends: on screen the working row
    // just goes away. Said through a polite live region (see [TurnFinishedNotice]).
    var wasWorking by remember(state.chatId) { mutableStateOf(state.working) }
    var turnFinished by remember(state.chatId) { mutableStateOf(false) }
    LaunchedEffect(state.working) {
        if (state.working) turnFinished = false else if (wasWorking) turnFinished = true
        wasWorking = state.working
    }
    val listState = rememberLazyListState()
    // A run of ordinary tool calls collapses to one line; inline diffs keep
    // their own file-change row, and a lone call shows its command. Grouped
    // again only when a row is added or rewritten, not for each streamed word.
    val rowCache = remember(state.chatId) { TranscriptRowCache() }
    val transcript = rowCache.rows(state)
    val settledRows = transcript.settled
    val rows = transcript.rows
    val answered = transcript.answered
    // Who a reply is from, said first when a screen reader reads it.
    val assistantName = DisplayLabels.backend(state.backend, catalog)
    val transcriptEmpty = transcript.isEmpty
    // The chat's agents by record key: the host rewrites one record per agent,
    // so each key is one row holding the agent's newest snapshot.
    val agentRuns = remember(settledRows) {
        settledRows.filterIsInstance<TranscriptRow.Subagent>()
            .mapNotNull { row -> row.run.key?.let { it to row.run } }
            .toMap()
    }
    val runningAgents = remember(settledRows) {
        settledRows.filterIsInstance<TranscriptRow.Subagent>()
            .map { it.run }
            // An agent chat opens as a chat of its own, not a tab here.
            .filter { it.state == SubagentState.RUNNING && it.chat == null }
    }
    // Kept while its agents are unchanged, so the tabs and header that take
    // it are not drawn again for every streamed word.
    val agents = remember(agentTabKeys, agentRuns, viewingAgent, model, workspacePane, sideBySide) {
        AgentTabs(
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
                // An agent shows where the chat does, so a tool in its place gives way.
                if (!sideBySide) workspacePane.pane = Pane.CHAT
                model.openAgent(key)
            },
            close = model::closeAgent,
            showChat = model::showChat,
        )
    }
    val shownAgent = viewingAgent?.let { agentRuns[it] }
    var trayOpen by rememberSaveable(state.chatId) { mutableStateOf(false) }
    val leadingItemCount =
        (if (state.hasOlderMessages) 1 else 0) + (if (state.error != null) 1 else 0)
    // The working row sits below the last message, so it is what "the bottom"
    // means while a turn runs.
    // The chat's agent chats at work or waiting on the user, for the tray.
    val trayWorkers = remember(agentChats) {
        agentChats.values.filter { it.working || com.restartfu.xd.model.AgentsTray.needsYou(it) }
    }
    // Between turns, the same place says what the agent still runs in the
    // background, unless the agents tray already lists it.
    val waitingOn = com.restartfu.xd.model.backgroundRowCount(state.working, runningAgents.size, backgroundTasks)
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

    // The newest finished replies are read into their parts in the
    // background as they arrive, so their rows have them on their first frame
    // rather than showing plain text for a moment.
    LaunchedEffect(rows) { newestReplies(rows).forEach(ReplyBodies::request) }
    val landingIndex = rememberUpdatedState(lastTranscriptIndex)
    LaunchedEffect(state.chatId, !transcriptEmpty) {
        if (!model.landed && !transcriptEmpty) {
            // The replies at the end read first, briefly: a row that changes
            // height after the jump would move the end landed on.
            withTimeoutOrNull(LANDING_PREPARE_MILLIS) { newestReplies(rows).map(ReplyBodies::request).awaitAll() }
            androidx.compose.runtime.withFrameNanos { }
            // Jumped, not animated: scrolling through a loaded history on open
            // is motion the reader did not ask for.
            listState.scrollToEnd(landingIndex.value)
            following.value = true
            model.landed = true
        }
    }

    // One follower for the visit, fed the newest end to keep in view. A word
    // arriving mid-scroll is picked up when that scroll ends, rather than
    // cancelling it and starting over for every word.
    val followTarget = rememberUpdatedState(
        FollowTarget(
            count = transcript.items.size + (if (transcript.segment != null) 1 else 0),
            lastText = state.lastVisibleItem?.text,
            leading = leadingItemCount,
            trailing = trailingItemCount,
            lastIndex = lastTranscriptIndex,
        ),
    )
    // The words streamed so far, which [state] leaves out: kept up in
    // composition, as the rest of the target is, by something small enough
    // to run for every word.
    val streamed = remember { mutableStateOf("") }
    StreamedText(streamingState, streamed)
    LaunchedEffect(listState, model) {
        var lastFollowed: Pair<FollowTarget?, Int?> = null to null
        // The newest message's drawn height too: a streaming reply is drawn
        // at a pace, so its last words can land after the text that woke
        // this. It is the row above the working row while a turn runs, and
        // the end stays the place scrolled to.
        snapshotFlow {
            val target = followTarget.value.let {
                if (it.lastText == LIVE_TEXT_ELSEWHERE) it.copy(lastText = streamed.value) else it
            }
            val newest = target.lastIndex - target.trailing
            val drawnHeight = listState.layoutInfo.visibleItemsInfo
                .firstOrNull { it.index == newest }?.size
            target to drawnHeight
        }.collect { (target, drawnHeight) ->
            // Only new output, or the newest message growing while in view,
            // moves the list; it leaving or entering the view is the reader's.
            val grew = target == lastFollowed.first &&
                drawnHeight != null &&
                (lastFollowed.second ?: Int.MAX_VALUE) < drawnHeight
            val changed = target != lastFollowed.first
            lastFollowed = target to drawnHeight
            if ((changed || grew) && model.landed && target.count > 0 && following.value) {
                try {
                    listState.scrollToEnd(target.lastIndex, animated = true)
                } catch (interrupted: CancellationException) {
                    // The reader's own scroll took over; only leaving stops following.
                    currentCoroutineContext().ensureActive()
                }
            }
        }
    }

    // A tool opening or closing beside the chat changes its width, and so
    // where its lines break. A reader who was at the end is kept at the end;
    // one who had scrolled away is left where they were.
    val toolBeside = sideBySide && pane != Pane.CHAT
    var toolWasBeside by remember { mutableStateOf(toolBeside) }
    LaunchedEffect(toolBeside) {
        if (toolBeside == toolWasBeside) return@LaunchedEffect
        toolWasBeside = toolBeside
        if (model.landed && following.value) {
            // Once the chat has been laid out at its new width.
            androidx.compose.runtime.withFrameNanos { }
            listState.scrollToEnd(followTarget.value.lastIndex)
        }
    }

    // What the reader writes into, under the conversation: the open agent's
    // composer, or the chat's with its agents tray. No wider than the reading
    // measure, so on a wide window it sits under the text it answers.
    val composerBar: @Composable () -> Unit = {
        // A notice on the app's snackbar shows above it, never over what is being written.
        Box(Modifier.fillMaxWidth().keepsSnackbarAbove(), contentAlignment = Alignment.TopCenter) {
            Box(Modifier.widthIn(max = ReadingMeasure + TranscriptEdge * 2)) {
                if (viewingAgent != null) {
                    val key = viewingAgent
                    AgentComposer(
                        run = shownAgent,
                        draft = key?.let { agentDrafts[it] }.orEmpty(),
                        onDraft = { text -> key?.let { model.updateAgentDraft(it, text) } },
                        sending = messagingAgent,
                        onSend = { shownAgent?.let(model::messageAgent) },
                        compact = sessionChats != null,
                    )
                } else {
                    Column {
                        AgentsTray(
                            running = runningAgents,
                            open = trayOpen,
                            onToggle = { trayOpen = !trayOpen },
                            onOpenAgent = agents.open,
                            compact = sessionChats != null,
                            // The chat's agent chats as the tree has them now: their
                            // rows may be far up the chat, or not loaded at all.
                            workers = trayWorkers,
                            onOpenChat = openChat,
                        )
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
            }
        }
    }
    // The chat's name, said as the page appears and when another chat takes its place.
    val chatName = sessionChats?.chats?.firstOrNull { it.id == sessionChats.selectedId }?.title?.ifBlank { null }
        ?: state.title.ifEmpty { "New chat" }
    Scaffold(
        modifier = Modifier.page(chatName),
        containerColor = GlassPage,
        contentColor = MaterialTheme.colorScheme.onSurface,
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
                    // Nothing while connected, so the header stays one row.
                    ConnectionLine()
                }
            } else Column {
                // One pane of glass for the whole header: the bars on it are
                // clear, so the title, the chat tabs and the views read as one.
                Column(Modifier.background(glassPane())) {
                productHeader()
                // A row rather than Material's top app bar, whose height is
                // fixed: enlarged text makes the title's two lines taller, and
                // the header grows with them instead of cutting them off.
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .safeTopPadding()
                        .heightIn(min = 64.dp)
                        .padding(start = 4.dp, end = 4.dp)
                        .testTag(TABLET_CHAT_HEADER_TAG),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    IconButton(onClick = goBack) {
                        Icon(painterResource(R.drawable.ic_back), contentDescription = "Back")
                    }
                    Column(Modifier.weight(1f).padding(start = 4.dp, end = 8.dp)) {
                        Text(
                            sessionTitle.ifBlank { state.title.ifEmpty { "Session" } },
                            modifier = Modifier.pageHeading(key = state.chatId),
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (projectName.isNotBlank()) Text(
                            projectName,
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        // Beside the title rather than in its second line, so
                        // each link is a button of full touch size.
                        if (linkedPullRequests != null) {
                            // Every pull request linked to the chat, as the desktop's header lists them.
                            LinkedPullRequestButtons(
                                pullRequests = linkedPullRequests,
                                limit = PullRequestChipLimit.TABLET,
                                open = openLink,
                                showAll = { showingPullRequests = true },
                            )
                        } else {
                            // The branch's pull request, as the desktop's session header shows it.
                            pullRequest?.let { url ->
                                PullRequestButton(
                                    label = pullRequestLabel(url),
                                    description = "Pull request ${pullRequestLabel(url)}",
                                    color = LocalXdSignals.current.link,
                                    decoration = TextDecoration.Underline,
                                    onClick = { openLink(url) },
                                )
                            }
                        }
                        // Beside the chat the tools are switches, not places: any of them opens next to it.
                        if (sideBySide) ToolSwitches(pane) { pane = it }
                        if (chatShown) {
                            TextButton(
                                onClick = { choosingModel = true },
                                // Says what it does, not only the model it names, as the composer's does.
                                modifier = Modifier.semantics {
                                    onClick(label = "Choose model") {
                                        choosingModel = true
                                        true
                                    }
                                },
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
                    }
                }
                sessionTabs(agents, showPullRequests)
                if (!sideBySide) TabRow(
                    selectedTabIndex = pane.ordinal,
                    containerColor = Color.Transparent,
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
                // Its own line under the header, as on every page.
                ConnectionLine()
            }
        },
        bottomBar = { if (!sideBySide && pane == Pane.CHAT) composerBar() },
    ) { scaffoldPadding ->
        TurnFinishedNotice(turnFinished)
        ChatPanes(
            sideBySide = sideBySide,
            pane = pane,
            panes = panes,
            padding = scaffoldPadding,
            composer = composerBar,
            tool = { shown ->
                when (shown) {
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
                            factory = FilesViewModel.Factory(unsavedFiles) { workspacePane.chat },
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
            },
        ) chat@{ padding ->
        if (viewingAgent != null) {
            val address = shownAgent?.messageAddress
            val sent = remember(transcript.items, address) {
                if (address == null) {
                    emptyList()
                } else {
                    transcript.items.filter { it.kind == TranscriptKind.USER }
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
                Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.TopCenter) {
                    AgentView(
                        run = shownAgent,
                        sent = sent,
                        queued = queued,
                        pendingAction = pending,
                        onAction = model::controlSubagent,
                        modifier = Modifier.widthIn(max = ReadingMeasure + TranscriptEdge * 2),
                    )
                }
            }
            return@chat
        }
        if (state.transcriptLoaded && !state.loading && !state.hasOlderMessages &&
            transcriptEmpty && state.error == null && !state.working
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
            return@chat
        }
        // The list is as wide as its pane, so it scrolls from its margins too;
        // what it holds stops at the reading measure and sits in the middle.
        BoxWithConstraints(Modifier.fillMaxSize().padding(padding)) {
        val side = readingInset(maxWidth, TranscriptEdge)
        LazyColumn(
            state = listState,
            modifier = Modifier
                .fillMaxSize()
                .testTag(TRANSCRIPT_TAG)
                .nestedScroll(followReader),
            contentPadding = PaddingValues(horizontal = side, vertical = TranscriptEdge),
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
            if (state.loading && transcriptEmpty) {
                item {
                    Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator()
                    }
                }
            }
            state.error?.let { error ->
                item {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            error,
                            // Told to a screen reader when it appears: nothing else says the chat failed.
                            modifier = Modifier.weight(1f).semantics { liveRegion = LiveRegionMode.Polite },
                            color = MaterialTheme.colorScheme.error,
                        )
                        // A transcript that did not load is read again from here.
                        if (state.loadFailed) {
                            TextButton(onClick = model::reload, enabled = !state.loading) { Text("Try again") }
                        }
                    }
                }
            }
            items(rows, key = { it.key }, contentType = ::rowContentType) { row ->
                when (row) {
                    is TranscriptRow.Single -> {
                        // xd's word that one of this chat's agents ended a turn.
                        val report = row.item.takeIf { it.kind == com.restartfu.xd.model.TranscriptKind.USER }
                            ?.let { com.restartfu.xd.model.AgentReport.parse(it.text) }
                        if (report != null) {
                            AgentReportLine(report, agentChats[report.chatId]) { openChat(report.chatId) }
                        } else {
                            TranscriptRow(row.item, model, assistantName, answered[row.item.id], liveText)
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
    }
}

/** The space between the conversation and the edge of its pane, and beside the composer. */
private val TranscriptEdge = 12.dp

/**
 * Lays the conversation out with the tool beside it or in its place.
 *
 * On a window wide enough for two panes the chat and its [composer] are
 * always here: a tool opens to their right and takes the room left once the
 * chat has its reading measure. The chat stays the same piece of the screen
 * whether a tool is open or not, so it keeps its place when one opens.
 * Anywhere else a tool takes the chat's place, as it always has, and the
 * composer is the page's bottom bar.
 */
@Composable
private fun ChatPanes(
    sideBySide: Boolean,
    pane: Pane,
    /** How the view in the chat's place changes, which Back's gesture can drive. */
    panes: PageTransition<Pane>,
    padding: PaddingValues,
    composer: @Composable () -> Unit,
    tool: @Composable (Pane) -> Unit,
    chat: @Composable (PaddingValues) -> Unit,
) {
    if (!sideBySide) {
        // The bars' room is taken here, so a tool that keeps clear of the
        // keyboard counts only what the keyboard covers beyond them.
        val still = reducedMotion()
        panes.transition.AnimatedContent(transitionSpec = { pageMotion(still, fadeThrough()) }) { shown ->
            if (shown != Pane.CHAT) {
                Box(Modifier.padding(padding).consumeWindowInsets(padding).page(shown.label)) { tool(shown) }
            } else {
                chat(padding)
            }
        }
        return
    }
    BoxWithConstraints(Modifier.fillMaxSize().padding(top = padding.calculateTopPadding())) {
        val open = pane != Pane.CHAT
        val chatWidth = chatPaneWidth(maxWidth, TranscriptEdge)
        Row(Modifier.fillMaxSize()) {
            Column(if (open) Modifier.width(chatWidth) else Modifier.weight(1f)) {
                Box(Modifier.weight(1f)) { chat(PaddingValues(0.dp)) }
                composer()
            }
            if (open) {
                VerticalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Box(
                    Modifier
                        .weight(1f)
                        .testTag(TOOL_PANE_TAG)
                        .page(pane.label)
                        .padding(bottom = padding.calculateBottomPadding())
                        .consumeWindowInsets(PaddingValues(bottom = padding.calculateBottomPadding())),
                ) { tool(pane) }
            }
        }
    }
}

/** The tool open beside the chat, and the conversation's list, for tests. */
internal const val TOOL_PANE_TAG = "tool-pane"

/** The tablet chat's header row, for tests. */
internal const val TABLET_CHAT_HEADER_TAG = "tablet-chat-header"
internal const val TRANSCRIPT_TAG = "transcript"

/**
 * Changes, Files and Terminal as switches in the tablet header: each opens its
 * tool beside the chat, and closes it again.
 */
@Composable
private fun ToolSwitches(pane: Pane, onPane: (Pane) -> Unit) {
    Row(
        Modifier.padding(horizontal = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Pane.entries.filter { it != Pane.CHAT }.forEach { tool ->
            val open = pane == tool
            FilterChip(
                selected = open,
                onClick = { onPane(if (open) Pane.CHAT else tool) },
                label = { Text(tool.label) },
                leadingIcon = if (open) {
                    { Icon(painterResource(R.drawable.ic_check), contentDescription = null, modifier = Modifier.size(18.dp)) }
                } else {
                    null
                },
            )
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
                    modifier = Modifier.weight(1f).announced(),
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
        if (state.todos.isNotEmpty()) {
            val completed = state.todos.count { it.status == TodoStatus.COMPLETED }
            Surface(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 160.dp),
                shape = MaterialTheme.shapes.medium,
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
                        Row(
                            // One stop for a screen reader, which hears how the task stands in words.
                            modifier = Modifier.semantics(mergeDescendants = true) {
                                stateDescription = when (todo.status) {
                                    TodoStatus.PENDING -> "Not started"
                                    TodoStatus.IN_PROGRESS -> "In progress"
                                    TodoStatus.COMPLETED -> "Done"
                                }
                            },
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            // A ring, a dot or a check, level with the first line of the task.
                            Box(
                                Modifier.height(with(LocalDensity.current) { 16.sp.toDp() }),
                                contentAlignment = Alignment.Center,
                            ) {
                                when (todo.status) {
                                    TodoStatus.PENDING -> BackgroundRing(MaterialTheme.colorScheme.onSurfaceVariant, size = 9.dp)
                                    TodoStatus.IN_PROGRESS -> Box(
                                        Modifier.size(9.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary),
                                    )
                                    TodoStatus.COMPLETED -> StatusMarkIcon(StatusMark.DONE, labelled = false)
                                }
                            }
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
            Text(it, modifier = Modifier.announced(), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
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
        val prompt = rememberComposerAsk(state, sending, model)
        if (compact) {
            CompactComposerRow(state, composer, sending, cancelling, steering, model, attachments.size, onModel, prompt) {
                picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
            }
            return@Column
        }
        // One rounded box, the way the desktop draws it: what waits on the
        // reader on top, then the message, then attach and send.
        Surface(
            shape = MaterialTheme.shapes.large,
            color = glassComposer(),
            border = BorderStroke(1.dp, controlBorder()),
        ) {
            Column {
                prompt.section?.invoke()
                Column(Modifier.padding(start = 4.dp, end = 8.dp, top = 2.dp, bottom = 6.dp)) {
                    TextField(
                        value = composer,
                        onValueChange = model::updateDraft,
                        // Named even once it holds text and the placeholder is gone.
                        modifier = Modifier.fillMaxWidth().semantics { contentDescription = COMPOSER_NAME },
                        placeholder = {
                            Text(prompt.placeholder ?: "Ask anything, or describe a task…", color = LocalXdSignals.current.faint)
                        },
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
                            onSend = {
                                when {
                                    prompt.answer != null -> prompt.answer.invoke()
                                    state.working -> model.enqueue()
                                    else -> model.send()
                                }
                            },
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
                            answer = prompt.answer?.takeIf { composer.isNotBlank() },
                            submittingAnswer = prompt.submitting,
                        )
                    }
                }
            }
        }
    }
}

/**
 * What the composer carries for whatever waits on the reader: the ask
 * section docked on top of it, the placeholder in its place, and what Send
 * does with a typed answer, where it differs from sending a message.
 */
private class ComposerAsk(
    val section: (@Composable () -> Unit)?,
    val placeholder: String?,
    val answer: (() -> Unit)?,
    /** Answers are on their way to the host: Send waits, busy, rather than becoming the stop. */
    val submitting: Boolean = false,
)

@Composable
private fun rememberComposerAsk(state: ChatState, sending: Boolean, model: ChatViewModel): ComposerAsk {
    val docked = remember(state) { dockedAsk(state) }
    val answers by model.interactionAnswers.collectAsStateWithLifecycle()
    val submitting by model.submittingAnswers.collectAsStateWithLifecycle()
    return when (docked) {
        null -> ComposerAsk(null, null, null)
        is DockedAsk.Interaction -> {
            val request = docked.request
            val showing = request.showing(answers[request.requestId].orEmpty())
            if (showing == null) {
                ComposerAsk({ ApprovalAsk(request, docked.waiting) { model.approve(request.requestId, it) } }, null, null)
            } else if (request.requestId in submitting) {
                // Nothing to answer until the host takes these or refuses them,
                // and the keyboard's Send must not queue the answer as a message.
                ComposerAsk(
                    section = { AgentQuestionAsk(request, showing, docked.waiting) { _, _ -> } },
                    placeholder = if (showing.question.secret) null else TYPED_ANSWER_PLACEHOLDER,
                    answer = {},
                    submitting = true,
                )
            } else {
                // A secret is typed in the ask itself, not in a draft the host keeps.
                val typed = !showing.sent && !showing.question.secret
                ComposerAsk(
                    section = {
                        AgentQuestionAsk(request, showing, docked.waiting) { id, answer ->
                            model.answerQuestion(request, id, answer)
                        }
                    },
                    placeholder = if (typed) TYPED_ANSWER_PLACEHOLDER else null,
                    answer = if (typed) ({ model.answerWithDraft(request, showing.question.id) }) else null,
                )
            }
        }
        // In place of the resume button: a chat stopped at its quota goes on
        // only with a new one. Messages sent meanwhile wait in its queue.
        DockedAsk.Quota -> {
            val quotaBusy by model.quotaBusy.collectAsStateWithLifecycle()
            val quotaError by model.quotaError.collectAsStateWithLifecycle()
            ComposerAsk(
                section = { QuotaAsk(state.quota, quotaBusy, quotaError, model::continueWithQuota) },
                placeholder = QUOTA_PLACEHOLDER,
                answer = null,
            )
        }
        // A typed answer to a tagged question is the next message, so Send
        // keeps its meaning; only the placeholder says it answers.
        is DockedAsk.Tagged -> ComposerAsk(
            section = { TaggedAsk(docked.ask, enabled = !sending, onAnswer = model::answer) },
            placeholder = if (docked.ask.acceptsInput) TYPED_ANSWER_PLACEHOLDER else null,
            answer = null,
        )
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
    prompt: ComposerAsk,
    onAttach: () -> Unit,
) {
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val faint = LocalXdSignals.current.faint
    Surface(
        shape = MaterialTheme.shapes.extraLarge,
        color = glassComposer(),
        border = BorderStroke(1.dp, controlBorder()),
    ) {
        Column {
            prompt.section?.invoke()
            // No padding of its own: each control sits in a 48dp slot that is
            // its touch target and no neighbour's, drawn at 40dp inside it.
            Row(verticalAlignment = Alignment.Bottom) {
                IconButton(
                    onClick = onAttach,
                    enabled = attached < Limits.MAX_IMAGES,
                    modifier = Modifier.size(ComposerSlot),
                ) {
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
                    // Named even once it holds text and the placeholder is gone.
                    modifier = Modifier.weight(1f).heightIn(min = ComposerSlot).semantics { contentDescription = COMPOSER_NAME },
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.onSurface),
                    maxLines = 6,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(
                        onSend = {
                            when {
                                prompt.answer != null -> prompt.answer.invoke()
                                state.working -> model.enqueue()
                                else -> model.send()
                            }
                        },
                    ),
                    decorationBox = { field ->
                        Box(Modifier.padding(end = 4.dp, top = 12.dp, bottom = 12.dp), contentAlignment = Alignment.CenterStart) {
                            if (composer.isEmpty()) {
                                Text(
                                    prompt.placeholder ?: "Ask anything…",
                                    style = MaterialTheme.typography.bodyLarge,
                                    color = faint,
                                    // An ask's placeholder is a sentence the reader needs whole.
                                    maxLines = if (prompt.placeholder != null) 2 else 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            field()
                        }
                    },
                )
                // The agent and model: the whole slot's height takes the tap,
                // and nothing is drawn around it until it is pressed.
                val modelLabel = DisplayLabels.model(state.model, state.backend, catalog)
                // With enlarged text the name would leave the message no room,
                // so the agent's mark stands alone and the name is spoken.
                val nameShown = LocalDensity.current.fontScale <= LARGE_TEXT_SCALE
                Row(
                    modifier = Modifier
                        .height(ComposerSlot)
                        .widthIn(min = ComposerSlot)
                        .clip(MaterialTheme.shapes.large)
                        .clickable(role = Role.Button, onClickLabel = "Choose model", onClick = onModel)
                        .semantics { if (!nameShown) contentDescription = modelLabel }
                        .padding(horizontal = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
                ) {
                    BackendIcon(state.backend, size = if (nameShown) 14.dp else 18.dp)
                    if (nameShown) Text(
                        modelLabel,
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.widthIn(max = 96.dp),
                    )
                }
                Box(Modifier.size(ComposerSlot), contentAlignment = Alignment.Center) {
                    SendButton(
                        state = state,
                        empty = composer.isBlank() && attached == 0,
                        sending = sending,
                        cancelling = cancelling,
                        steering = steering,
                        model = model,
                        size = 40.dp,
                        answer = prompt.answer?.takeIf { composer.isNotBlank() },
                        submittingAnswer = prompt.submitting,
                    )
                }
            }
        }
    }
}

/** The touch target each control of the one-row composer gets, around its 40dp drawing. */
internal val ComposerSlot = 48.dp

/** The font scale past which rows drop words they have no room for. */
internal const val LARGE_TEXT_SCALE = 1.3f

/** What a screen reader is told when the turn ends. */
internal const val TURN_FINISHED_ANNOUNCEMENT = "Reply finished"

/**
 * Tells a screen reader that a reply finished, as a polite live region: the
 * words are set when the turn ends, and cleared when the next one starts so
 * the next ending is a change to tell again. Nothing is drawn.
 */
@Composable
private fun TurnFinishedNotice(finished: Boolean) {
    Box(
        Modifier
            .size(1.dp)
            .testTag(TURN_FINISHED_TAG)
            .semantics {
                liveRegion = LiveRegionMode.Polite
                if (finished) contentDescription = TURN_FINISHED_ANNOUNCEMENT
            },
    )
}

/** The live region that says a reply finished, for tests. */
internal const val TURN_FINISHED_TAG = "turn-finished"

/** What a screen reader calls the chat's composer, whatever it holds. */
internal const val COMPOSER_NAME = "Message"

/** What the composer's one round button does. */
internal enum class SendAction { SEND, STOP, RESUME, SEND_ANSWER, ANSWER_BUSY }

/**
 * The button's action. A typed answer to the agent's question is sent even
 * while the turn waiting on it runs, and while that answer is on its way the
 * button stays a busy send: becoming the stop then would let a second tap
 * cancel the very turn the answer is for.
 */
internal fun sendAction(
    working: Boolean,
    resumable: Boolean,
    quotaReached: Boolean,
    empty: Boolean,
    answerTyped: Boolean,
    submittingAnswer: Boolean,
): SendAction = when {
    submittingAnswer -> SendAction.ANSWER_BUSY
    answerTyped -> SendAction.SEND_ANSWER
    working -> SendAction.STOP
    // A chat stopped at its quota is continued from its quota ask instead.
    resumable && !quotaReached && empty -> SendAction.RESUME
    else -> SendAction.SEND
}

/**
 * One round button that sends and then stops, as on the desktop: an arrow in
 * the accent while it is yours to send, a neutral stop while the turn is the
 * assistant's to finish. With nothing typed after a turn was cut short, the
 * same button continues it, as the desktop's play button does. See
 * [sendAction] for answers to the agent's questions.
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
    /** Sends what is typed as the answer to the question showing, when one takes it. */
    answer: (() -> Unit)? = null,
    /** Answers to the agent's question are on their way. */
    submittingAnswer: Boolean = false,
) {
    val action = sendAction(
        working = state.working,
        resumable = state.resumable,
        quotaReached = state.quotaReached,
        empty = empty,
        answerTyped = answer != null,
        submittingAnswer = submittingAnswer,
    )
    val stopping = action == SendAction.STOP
    val resuming = action == SendAction.RESUME
    FilledIconButton(
        onClick = {
            when (action) {
                SendAction.ANSWER_BUSY -> Unit
                SendAction.SEND_ANSWER -> answer?.invoke()
                SendAction.STOP -> model.cancel()
                SendAction.RESUME -> model.resumeTurn()
                SendAction.SEND -> model.send()
            }
        },
        enabled = when (action) {
            SendAction.ANSWER_BUSY -> false
            SendAction.SEND_ANSWER -> true
            SendAction.STOP -> !cancelling && !steering
            SendAction.RESUME -> !sending
            // The host accepts text, images, or both.
            SendAction.SEND -> !sending && !empty
        },
        modifier = Modifier.size(size),
        colors = if (stopping) {
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
                when (action) {
                    SendAction.STOP -> R.drawable.ic_stop
                    SendAction.RESUME -> R.drawable.ic_play
                    else -> R.drawable.ic_send
                },
            ),
            contentDescription = when {
                action == SendAction.ANSWER_BUSY -> "Sending answer"
                action == SendAction.SEND_ANSWER -> "Send answer"
                cancelling -> "Stopping"
                stopping -> "Stop"
                sending -> "Sending"
                resuming -> "Continue where it left off"
                else -> "Send"
            },
            modifier = Modifier.size(if (stopping || resuming) size / 3 else size * 5 / 12),
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
                    colors = xdFieldColors(),
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
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (working) {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Steering stops the running turn and starts this instead.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
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
    if (item.kind == TranscriptKind.ASSISTANT) {
        AssistantBody(item, model)
        return
    }
    val parts = remember(item.text, item.kind) { ImageReference.parts(item.text) }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        parts.forEach { part ->
            when (part) {
                is MessagePart.Image -> MessageImage(model, part.path)
                is MessagePart.Prose -> when (item.kind) {
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
 * An assistant's reply: its pictures, and its prose as rendered Markdown.
 *
 * One still arriving is read again as it grows, on a background thread, and
 * only what changed is drawn again: see [assistantBody]. A finished one is
 * read once, in the background, and kept for every time its row is shown
 * again (see [ReplyBodies]); for the moment before that it shows as plain text.
 */
@Composable
internal fun AssistantBody(item: TranscriptItem, model: ChatViewModel) {
    val parts = if (item.live) {
        rememberOffMain(item.text) { text, previous: List<BodyPart>? -> assistantBody(text, live = true, previous) }
            ?.also { LiveHandoff.remember(item.text, it) }
            // Until the first reading is ready, what an earlier frame of this reply rendered.
            ?: LiveHandoff.forText(item.text)
    } else {
        rememberPrepared(ReplyBodies, ReplyKey(item.id, item.text)) ?: LiveHandoff.forText(item.text)
    }
    if (parts == null) {
        Text(item.text)
        return
    }
    CompositionLocalProvider(LocalStreaming provides item.live) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            parts.forEach { part ->
                when (part) {
                    is BodyPart.Image -> MessageImage(model, part.path)
                    is BodyPart.Prose -> AssistantProse(part.document, messageId = item.id)
                }
            }
        }
    }
}

/**
 * Assistant prose with its client-side presentation blocks lifted out.
 *
 * `<ask>` becomes the existing question controls, `<analysis>` is an initially
 * closed disclosure, and `<summary>` is ordinary visible Markdown.
 */
@Composable
private fun AssistantProse(
    document: ProseDocument,
    messageId: String,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        document.sections.forEachIndexed { index, section ->
            when (section.kind) {
                AssistantSectionKind.NORMAL ->
                    section.blocks?.takeIf { it.isNotEmpty() }?.let { MarkdownBlocks(it) }
                AssistantSectionKind.ANALYSIS ->
                    AnalysisDisclosure(messageId, index, section)
            }
        }
        document.question?.let { Text(it, fontWeight = FontWeight.Bold) }
    }
}

@Composable
private fun AnalysisDisclosure(
    messageId: String,
    ordinal: Int,
    section: ProseSection,
) {
    val key = "$messageId-analysis-$ordinal"
    var expanded by rememberSaveable(key) { mutableStateOf(false) }

    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        shape = MaterialTheme.shapes.medium,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .disclosure(expanded) { expanded = !expanded }
                    .heightIn(min = 48.dp)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "Analysis",
                    modifier = Modifier.weight(1f),
                    color = LocalXdSignals.current.faint,
                    style = MaterialTheme.typography.bodySmall,
                )
                DisclosureChevron(expanded)
            }
            if (expanded && section.text.isNotBlank()) {
                val inset = Modifier.padding(horizontal = 12.dp, vertical = 8.dp)
                section.blocks?.let { MarkdownBlocks(it, inset) } ?: MarkdownText(section.text, inset)
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

/** How long landing in a chat waits for its newest replies to be read. */
private const val LANDING_PREPARE_MILLIS = 400L

/** How many of the newest finished replies are read ahead of being drawn. */
private const val PREPARED_REPLIES = 40

/** The newest finished replies in [rows], newest first, as [ReplyBodies] keys them. */
private fun newestReplies(rows: List<TranscriptRow>): List<ReplyKey> =
    rows.asReversed().asSequence()
        .mapNotNull { (it as? TranscriptRow.Single)?.item }
        .filter { it.kind == com.restartfu.xd.model.TranscriptKind.ASSISTANT && !it.live }
        .take(PREPARED_REPLIES)
        .map { ReplyKey(it.id, it.text) }
        .toList()

/** What the chat's follower keeps in view: the end, as of the newest output. */
private data class FollowTarget(
    val count: Int,
    val lastText: String?,
    val leading: Int,
    val trailing: Int,
    val lastIndex: Int,
)

/** How often a streaming reply is drawn again. */
private const val LIVE_TEXT_INTERVAL_MILLIS = 80L

/**
 * [text] as last drawn: its newest value, at most once per
 * [LIVE_TEXT_INTERVAL_MILLIS]. The first value shows at once.
 */
@Composable
private fun rememberPacedText(text: () -> String): String {
    val latest = rememberUpdatedState(text)
    // Read without subscribing: the row is drawn again when [shown] moves on,
    // at its pace, not whenever the text does.
    var shown by remember { mutableStateOf(Snapshot.withoutReadObservation { text() }) }
    LaunchedEffect(Unit) {
        snapshotFlow { latest.value() }.collect { value ->
            // The reply's last words stay up until its finished row takes over.
            if (value.isNotEmpty()) {
                shown = value
                delay(LIVE_TEXT_INTERVAL_MILLIS)
            }
        }
    }
    return shown
}

/**
 * Stands in for the words of the reply streaming in, in the state the screen
 * as a whole reads: see [withoutLiveText].
 */
internal const val LIVE_TEXT_ELSEWHERE = "\u2026"

/**
 * The chat without the words of the reply still streaming in, so that what
 * reads it is not drawn again for each of them. Whether a reply is streaming,
 * and everything else, is as it was; the words are read where they are drawn.
 */
internal fun ChatState.withoutLiveText(): ChatState =
    if (working && liveSegment.isNotEmpty()) copy(liveSegment = LIVE_TEXT_ELSEWHERE, turnSequence = 0) else this

/** Keeps [into] at the words streamed so far, for the follower. */
@Composable
private fun StreamedText(state: State<ChatState>, into: MutableState<String>) {
    into.value = state.value.liveSegment
}

/** Rows drawn alike, so the list can reuse one's layout for another. */
private fun rowContentType(row: TranscriptRow): String = when (row) {
    is TranscriptRow.Single -> "single-${row.item.kind}"
    is TranscriptRow.Tools -> "tools"
    is TranscriptRow.AccountSwitch -> "account-switch"
    is TranscriptRow.Pipeline -> "pipeline"
    is TranscriptRow.Subagent -> "subagent"
}

/**
 * A run of tool calls behind one line.
 *
 * Expanding shows each call's command. Their detail stays behind its own
 * toggle, so opening a run of ten does not unfold ten command outputs at once.
 */
@Composable
private fun ToolGroupRow(row: TranscriptRow.Tools) {
    var expanded by rememberSaveable(row.key) { mutableStateOf(false) }

    Surface(
        color = MaterialTheme.colorScheme.surfaceContainer,
        shape = MaterialTheme.shapes.medium,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(Modifier.fillMaxWidth()) {
            // The line itself is the button; the calls under it have their own.
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .disclosure(expanded) { expanded = !expanded }
                    .heightIn(min = 48.dp)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    row.label,
                    modifier = Modifier.weight(1f),
                    color = LocalXdSignals.current.faint,
                    style = MaterialTheme.typography.bodySmall,
                )
                DisclosureChevron(expanded)
            }
            if (expanded) {
                Column(Modifier.padding(start = 14.dp, end = 14.dp, bottom = 6.dp)) {
                    row.items.forEach { ToolRow(it, nested = true) }
                }
            }
        }
    }
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
        shape = if (nested) MaterialTheme.shapes.small else MaterialTheme.shapes.medium,
        border = if (nested) null else BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(Modifier.fillMaxWidth()) {
            // A full-height row whether it opens or not, so a run of calls keeps one rhythm.
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .disclosure(expanded, enabled = detail != null) { expanded = !expanded }
                    .heightIn(min = 48.dp)
                    .padding(horizontal = if (nested) 0.dp else 14.dp, vertical = if (nested) 5.dp else 10.dp),
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
                if (detail != null) DisclosureChevron(expanded)
            }
            if (expanded && detail != null) {
                Box(Modifier.padding(start = if (nested) 0.dp else 14.dp, end = if (nested) 0.dp else 14.dp, bottom = 10.dp)) {
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
private fun TranscriptRow(
    row: TranscriptItem,
    model: ChatViewModel,
    /** The chat's assistant by name, as a screen reader says who a reply is from. */
    assistantName: String,
    answering: String? = null,
    /** The words of the reply streaming in, as they stand now. */
    liveText: () -> String = { row.text },
) {
    // The reply still streaming in is drawn at a steady pace rather than
    // parsed and laid out again for every word.
    val item = if (row.id == ChatState.LIVE_SEGMENT_ID) {
        val shown = rememberPacedText { if (row.text == LIVE_TEXT_ELSEWHERE) liveText() else row.text }
        remember(row.id, shown) { row.copy(text = shown) }
    } else {
        row
    }
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
        // text keeps its label, so it cannot be mistaken for the agent. A
        // screen reader is told the name instead.
        if (item.kind == TranscriptKind.ASSISTANT) {
            SpokenMessage(assistantName, Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
                SelectableMessageBody(item, model)
            }
            return
        }
        Column(Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
            item.label?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.labelSmall,
                    color = LocalXdSignals.current.faint,
                )
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
        // The bubble's side and tint say whose it is; a screen reader is told.
        SpokenMessage(YOU, Modifier.fillMaxWidth(0.86f)) {
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
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
                shape = MaterialTheme.shapes.large,
            ) {
                Column(Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
                    SelectableMessageBody(shown, model)
                }
            }
        }
        }
    }
}

/** Who the reader's own messages are from, to a screen reader. */
internal const val YOU = "You"

/**
 * One message as one stop for a screen reader, starting with who it is from.
 * The name is a node of its own under the message, first in it, so it is
 * read before the words; the stop itself carries no text, so a screen reader
 * reads all it holds. Links, pictures and anything else that acts on its own
 * stay stops of their own, and selection is untouched.
 */
@Composable
private fun SpokenMessage(speaker: String, modifier: Modifier, content: @Composable () -> Unit) {
    Box(modifier.semantics(mergeDescendants = true) {}) {
        Box(Modifier.matchParentSize().semantics { text = AnnotatedString(speaker) })
        content()
    }
}

/** The caption on a message the reader sent one of the agents, as on the desktop. */
internal fun agentRelayLabel(description: String): String = "To $description"

/** The caption on a message sent in reply to a question, as on the desktop. */
internal fun answeredQuestionLabel(question: String): String = "Answering \u201c${question.trim()}\u201d"
