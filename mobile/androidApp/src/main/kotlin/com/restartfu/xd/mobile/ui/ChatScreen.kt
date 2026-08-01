package com.restartfu.xd.mobile.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.model.ToolText
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.syntax.CodeBlocks

/** The desktop's conversation, diff, files and terminal panes, as tabs. */
internal enum class Pane(val label: String) {
    CHAT("Chat"),
    DIFF("Diff"),
    FILES("Files"),
    TERMINAL("Terminal"),
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ChatScreen(
    model: ChatViewModel,
    goBack: () -> Unit,
) {
    val state by model.state.collectAsStateWithLifecycle()
    val sending by model.sending.collectAsStateWithLifecycle()
    val cancelling by model.cancelling.collectAsStateWithLifecycle()
    val steering by model.steering.collectAsStateWithLifecycle()
    val composer by model.draft.collectAsStateWithLifecycle()

    // The desktop shows these beside the conversation. A phone has room for
    // one at a time, so they are tabs over the same chat.
    var pane by rememberSaveable(state.chatId) { mutableStateOf(Pane.CHAT) }
    var choosingModel by rememberSaveable(state.chatId) { mutableStateOf(false) }

    if (choosingModel) {
        ModelPicker(
            model = model,
            currentBackend = state.backend,
            currentModel = state.model,
            onDismiss = { choosingModel = false },
        )
    }
    val listState = rememberLazyListState()
    val items = state.visibleItems
    val leadingItemCount =
        (if (state.hasOlderMessages) 1 else 0) + (if (state.error != null) 1 else 0)
    val lastTranscriptIndex = leadingItemCount + items.lastIndex
    val atBottom by remember(items.size, leadingItemCount) {
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

    LaunchedEffect(items.size, items.lastOrNull()?.text, leadingItemCount) {
        if (landed && items.isNotEmpty() && atBottom) {
            listState.animateScrollToItem(lastTranscriptIndex)
        }
    }

    Scaffold(
        topBar = {
            Column {
                TopAppBar(
                    title = {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            BackendIcon(state.backend)
                            if (state.backend.isNotEmpty()) Spacer(Modifier.width(10.dp))
                            Text(
                                state.title.ifEmpty { "Chat" },
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    },
                    navigationIcon = { TextButton(onClick = goBack) { Text("Back") } },
                    actions = {
                        if (pane == Pane.CHAT) {
                            TextButton(onClick = { choosingModel = true }) {
                                Text(
                                    state.model?.takeIf { it.isNotBlank() } ?: "Model",
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.widthIn(max = 130.dp),
                                )
                            }
                        }
                    },
                )
                TabRow(selectedTabIndex = pane.ordinal) {
                    Pane.entries.forEach { candidate ->
                        Tab(
                            selected = pane == candidate,
                            onClick = { pane = candidate },
                            text = { Text(candidate.label) },
                        )
                    }
                }
            }
        },
        bottomBar = {
            if (pane == Pane.CHAT) {
                Composer(
                    state = state,
                    composer = composer,
                    sending = sending,
                    cancelling = cancelling,
                    steering = steering,
                    model = model,
                )
            }
        },
    ) { padding ->
        if (pane != Pane.CHAT) {
            Box(Modifier.padding(padding)) {
                when (pane) {
                    Pane.DIFF -> DiffPaneContent(
                        viewModel(
                            key = "diff-${state.chatId}",
                            factory = DiffViewModel.Factory(model.session),
                        ),
                    )
                    Pane.FILES -> FilesPaneContent(
                        viewModel(
                            key = "files-${state.chatId}",
                            factory = FilesViewModel.Factory(model.session),
                        ),
                    )
                    Pane.TERMINAL -> {
                        val terminal: TerminalViewModel = viewModel(
                            key = "terminal-${state.chatId}",
                            factory = TerminalViewModel.Factory(
                                model.session,
                                state.chatId,
                            ),
                        )
                        LaunchedEffect(terminal) {
                            model.client.terminalEvents.collect(terminal::onEvent)
                        }
                        TerminalPaneContent(terminal)
                    }
                    else -> Unit
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
            items(items, key = TranscriptItem::id) { item ->
                TranscriptRow(item)
            }
        }
    }
}

/**
 * One turn per chat is a daemon rule, so while a turn runs the send action
 * becomes Queue and Cancel appears beside it.
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
            .imePadding()
            .padding(12.dp),
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
                        Surface(
                            color = MaterialTheme.colorScheme.surface,
                            shape = MaterialTheme.shapes.small,
                            modifier = Modifier
                                .align(Alignment.TopEnd)
                                .clickable { model.removeAttachment(index) },
                        ) {
                            Text("✕", modifier = Modifier.padding(horizontal = 5.dp))
                        }
                    }
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
        Row(verticalAlignment = Alignment.Bottom) {
            TextButton(
                onClick = {
                    picker.launch(
                        PickVisualMediaRequest(
                            ActivityResultContracts.PickVisualMedia.ImageOnly,
                        ),
                    )
                },
                enabled = attachments.size < Limits.MAX_IMAGES,
            ) {
                Text("+")
            }
            OutlinedTextField(
                value = composer,
                onValueChange = model::updateDraft,
                modifier = Modifier.weight(1f),
                label = { Text("Message") },
                minLines = 1,
                maxLines = 5,
            )
            Spacer(Modifier.width(8.dp))
            if (state.working) {
                Button(
                    onClick = model::enqueue,
                    enabled = !sending && composer.isNotBlank(),
                ) {
                    Text(if (sending) "Queueing…" else "Queue")
                }
                TextButton(
                    onClick = model::cancel,
                    enabled = !cancelling && !steering,
                ) {
                    Text(if (cancelling) "Cancelling…" else "Cancel")
                }
            } else {
                Button(
                    onClick = model::send,
                    // The daemon accepts text, images, or both.
                    enabled = !sending && (composer.isNotBlank() || attachments.isNotEmpty()),
                ) {
                    Text(if (sending) "Sending…" else "Send")
                }
            }
        }
    }
}

/**
 * Edit, steer or drop one queued message.
 *
 * Steering is only offered while a turn is running, because that is the only
 * time it means anything: the daemon promotes the message and stops the turn
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
 * Message text, with fenced code blocks lifted into syntax-coloured cards.
 *
 * Only fences are interpreted; the rest of the Markdown is left as written,
 * because half-rendered Markdown reads worse than none.
 */
@Composable
private fun MessageBody(item: TranscriptItem) {
    val segments = remember(item.text) { CodeBlocks.segments(item.text) }
    val plain = item.kind == TranscriptKind.SYSTEM

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        segments.forEach { segment ->
            if (segment.code) {
                Surface(
                    color = MaterialTheme.colorScheme.surfaceVariant,
                    shape = MaterialTheme.shapes.small,
                ) {
                    CodeText(
                        segment.text,
                        segment.language,
                        modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
                    )
                }
            } else {
                Text(
                    segment.text,
                    fontFamily = if (plain) FontFamily.Monospace else FontFamily.Default,
                )
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
private fun ToolRow(item: TranscriptItem) {
    val summary = remember(item.text) { ToolText.summary(item.text) }
    val detail = remember(item.text) { ToolText.detail(item.text) }
    val isPatch = remember(item.text) { ToolText.patch(item.text) != null }
    var expanded by rememberSaveable(item.id) { mutableStateOf(false) }

    Surface(
        color = MaterialTheme.colorScheme.surfaceVariant,
        shape = MaterialTheme.shapes.small,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .let { if (detail == null) it else it.clickable { expanded = !expanded } }
                .padding(horizontal = 12.dp, vertical = 8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (detail != null) {
                    Text(if (expanded) "▾" else "▸")
                    Spacer(Modifier.width(8.dp))
                }
                Text(
                    summary,
                    modifier = Modifier.weight(1f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    fontFamily = if (isPatch) FontFamily.Monospace else FontFamily.Default,
                )
            }
            if (expanded && detail != null) {
                Spacer(Modifier.height(8.dp))
                if (isPatch) {
                    DiffText(detail)
                } else {
                    Text(
                        detail,
                        modifier = Modifier.horizontalScroll(rememberScrollState()),
                        fontFamily = FontFamily.Monospace,
                        style = MaterialTheme.typography.bodySmall,
                        softWrap = false,
                    )
                }
            }
        }
    }
}

@Composable
private fun TranscriptRow(item: TranscriptItem) {
    if (item.kind == TranscriptKind.TOOL) {
        ToolRow(item)
        return
    }
    val user = item.kind == TranscriptKind.USER
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = if (user) Arrangement.End else Arrangement.Start,
    ) {
        Card(Modifier.fillMaxWidth(if (user) 0.86f else 1f)) {
            Column(Modifier.padding(12.dp)) {
                item.label?.let {
                    Text(it, style = MaterialTheme.typography.labelSmall)
                }
                MessageBody(item)
            }
        }
    }
}
