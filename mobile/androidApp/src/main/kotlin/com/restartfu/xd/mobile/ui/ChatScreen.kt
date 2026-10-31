package com.restartfu.xd.mobile.ui

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
import androidx.compose.foundation.lazy.LazyColumn
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
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
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.restartfu.xd.mobile.AndroidSpeechSpeaker
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.model.AskBlock
import com.restartfu.xd.model.AssistantSection
import com.restartfu.xd.model.AssistantSectionKind
import com.restartfu.xd.model.AssistantSections
import com.restartfu.xd.model.SubagentState
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
    settings: MobileSettings,
    goBack: () -> Unit,
    sessionTitle: String = "",
    projectName: String = "",
    workspaceId: String = model.state.value.chatId,
    terminalChatId: String = model.state.value.chatId,
    workspaceOwner: ViewModelStoreOwner? = null,
    productHeader: @Composable () -> Unit = {},
    sessionTabs: @Composable () -> Unit = {},
) {
    val state by model.state.collectAsStateWithLifecycle()
    val sending by model.sending.collectAsStateWithLifecycle()
    val cancelling by model.cancelling.collectAsStateWithLifecycle()
    val steering by model.steering.collectAsStateWithLifecycle()
    val subagentActions by model.subagentActions.collectAsStateWithLifecycle()
    val composer by model.draft.collectAsStateWithLifecycle()
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val speechEnabled by settings.speechEnabled.collectAsStateWithLifecycle()

    SpeechOutput(model, speechEnabled)
    LaunchedEffect(speechEnabled) {
        if (!speechEnabled) model.resetSpeech()
    }

    // The desktop shows these beside the conversation. A phone has room for
    // one at a time, so they are tabs over the same chat.
    val workspacePane: WorkspacePaneModel = viewModel(
        key = "workspace-pane-$workspaceId",
        viewModelStoreOwner = workspaceOwner ?: checkNotNull(androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner.current),
    )
    workspacePane.chat = model.session
    var pane by workspacePane::pane
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
    val listState = rememberLazyListState()
    val items = state.visibleItems

    // A run of ordinary tool calls collapses to one line; inline diffs keep
    // their own file-change row, and a lone call shows its command.
    val rows = remember(items) { ToolGrouping.rows(items) }
    val leadingItemCount =
        (if (state.hasOlderMessages) 1 else 0) + (if (state.error != null) 1 else 0)
    // The working row sits below the last message, so it is what "the bottom"
    // means while a turn runs.
    val trailingItemCount = if (state.working) 1 else 0
    val lastTranscriptIndex = leadingItemCount + rows.lastIndex + trailingItemCount
    val atBottom by remember(rows.size, leadingItemCount, trailingItemCount) {
        derivedStateOf {
            val last = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: -1
            last >= lastTranscriptIndex - 1
        }
    }

    // Opening a chat lands on the newest message, not the oldest. This cannot
    // be inferred from the list state: by the time an effect runs the items
    // are already laid out, so "nothing measured yet" reads as a full list and
    // the jump never happens. Track the first load for this chat instead.
    // Saved, so a rotation does not re-land and throw away the scroll position
    // rememberLazyListState just restored.
    var landed by rememberSaveable(state.chatId) { mutableStateOf(false) }

    LaunchedEffect(state.chatId, items.isNotEmpty()) {
        if (!landed && items.isNotEmpty()) {
            // Jumped, not animated: scrolling through a loaded history on open
            // is motion the reader did not ask for.
            listState.scrollToItem(lastTranscriptIndex)
            landed = true
        }
    }

    LaunchedEffect(items.size, items.lastOrNull()?.text, leadingItemCount, trailingItemCount) {
        if (landed && items.isNotEmpty() && atBottom) {
            listState.animateScrollToItem(lastTranscriptIndex)
        }
    }

    Scaffold(
        containerColor = MaterialTheme.colorScheme.surface,
        topBar = {
            Column {
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
                                if (projectName.isNotBlank()) Text(
                                    projectName,
                                    style = MaterialTheme.typography.labelMedium,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
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
                sessionTabs()
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
            if (pane == Pane.CHAT) {
                Column {
                    state.interactions.firstOrNull()?.let { request -> InteractionCard(request, model) }
                Composer(
                    state = state,
                    composer = composer,
                    sending = sending,
                    cancelling = cancelling,
                    steering = steering,
                    model = model,
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
        if (!state.loading && items.isEmpty() && state.error == null && !state.working) {
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
                .padding(padding),
            contentPadding = PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
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
                    is TranscriptRow.Single -> TranscriptRow(row.item, model)
                    is TranscriptRow.Tools -> ToolGroupRow(row)
                    is TranscriptRow.Pipeline -> PipelineCard(row.run, model.client)
                    is TranscriptRow.Subagent -> {
                        val key = row.run.key
                        val pending = key?.let { subagentActions[it] }
                        if (key != null && pending != null) {
                            val settled = row.run.state != SubagentState.RUNNING ||
                                (pending == "Moving to background" && row.run.background)
                            LaunchedEffect(settled) { if (settled) model.settleSubagentAction(key) }
                        }
                        SubagentCard(row.run, pending, model::controlSubagent)
                    }
                }
            }
            if (state.working) {
                item(key = "working") { WorkingRow(state.startedAtMillis) }
            }
        }
    }
}

@Composable
private fun SpeechOutput(
    model: ChatViewModel,
    enabled: Boolean,
) {
    if (!enabled) return

    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val speaker = remember(context) { AndroidSpeechSpeaker(context) }

    DisposableEffect(lifecycleOwner, speaker) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) speaker.stop()
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            speaker.shutdown()
        }
    }
    LaunchedEffect(model, speaker, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            model.resetSpeech()
            model.speech.collect { speaker.speak(it) }
        }
    }
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
) {
    val context = LocalContext.current
    val attachments by model.attachments.collectAsStateWithLifecycle()
    val catalog by model.catalog.collectAsStateWithLifecycle()
    val attachmentError by model.attachmentError.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<Pair<Int, String>?>(null) }

    editing?.let { (index, original) ->
        QueuedMessageDialog(
            original = original,
            working = state.working,
            onDismiss = { editing = null },
            onSave = { text ->
                editing = null
                if (text != original) model.editQueued(index, original, text)
            },
            onSteer = { text ->
                editing = null
                model.steerQueued(index, original, text)
            },
            onDrop = {
                editing = null
                model.dropQueued(index)
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
            .padding(horizontal = 12.dp, vertical = 10.dp),
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
                        onClick = { editing = index to queued },
                        label = {
                            Text(
                                queued,
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
                    // One round button that sends and then stops, as on the
                    // desktop: an arrow in the accent while it is yours to send,
                    // a neutral stop while the turn is the assistant's to finish.
                    FilledIconButton(
                        onClick = { if (state.working) model.cancel() else model.send() },
                        enabled = if (state.working) {
                            !cancelling && !steering
                        } else {
                            // The host accepts text, images, or both.
                            !sending && (composer.isNotBlank() || attachments.isNotEmpty())
                        },
                        modifier = Modifier.size(48.dp),
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
                                if (state.working) R.drawable.ic_stop else R.drawable.ic_send,
                            ),
                            contentDescription = when {
                                cancelling -> "Stopping"
                                state.working -> "Stop"
                                sending -> "Sending"
                                else -> "Send"
                            },
                            modifier = Modifier.size(if (state.working) 16.dp else 20.dp),
                        )
                    }
                }
            }
        }
    }
}

/**
 * Edit, steer or drop one queued message.
 *
 * Steering is only offered while a turn is running, because that is the only
 * time it means anything: the host promotes the message and stops the turn
 * so the agent takes it up instead of finishing first.
 */
@Composable
private fun QueuedMessageDialog(
    original: String,
    working: Boolean,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
    onSteer: (String) -> Unit,
    onDrop: () -> Unit,
) {
    var text by rememberSaveable(original) { mutableStateOf(original) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Queued message") },
        text = {
            Column {
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Message") },
                    minLines = 2,
                    maxLines = 6,
                )
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
                        enabled = text.isNotBlank(),
                    ) { Text("Steer") }
                }
                TextButton(
                    onClick = { onSave(text) },
                    enabled = text.isNotBlank(),
                ) { Text("Save") }
            }
        },
        dismissButton = {
            Row {
                TextButton(onClick = onDrop) { Text("Drop") }
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
    val parts = remember(item.text) { ImageReference.parts(item.text) }

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
                        Text(part.text, fontFamily = XdMonoFamily)
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

private fun rowKey(row: TranscriptRow): String = when (row) {
    is TranscriptRow.Single -> row.item.id
    // Keyed on the run's first call, which is stable as the run grows.
    is TranscriptRow.Tools -> "tools-${row.items.first().id}"
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
 * One message.
 *
 * Only the reader's own messages get a bubble, as on the desktop. A reply is
 * the page: it is most of what is on screen, it is the thing being read, and
 * carding it too would leave the two indistinguishable — a wall of identical
 * boxes where the only difference is which side is narrower.
 */
@Composable
private fun TranscriptRow(item: TranscriptItem, model: ChatViewModel) {
    if (item.kind == TranscriptKind.TOOL) {
        ToolRow(item)
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
    Box(
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        contentAlignment = Alignment.CenterEnd,
    ) {
        Box(Modifier.fillMaxWidth(0.86f), contentAlignment = Alignment.CenterEnd) {
            Surface(
                // Tinted rather than the page colour: against plain reply text
                // the bubble has to carry the distinction on its own.
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                contentColor = MaterialTheme.colorScheme.onSurface,
                shape = RoundedCornerShape(20.dp),
            ) {
                Column(Modifier.padding(horizontal = 14.dp, vertical = 10.dp)) {
                    SelectableMessageBody(item, model)
                }
            }
        }
    }
}

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
