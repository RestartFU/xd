package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.TranscriptKind

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ChatScreen(
    model: ChatViewModel,
    goBack: () -> Unit,
) {
    val state by model.state.collectAsStateWithLifecycle()
    val sending by model.sending.collectAsStateWithLifecycle()
    val cancelling by model.cancelling.collectAsStateWithLifecycle()
    val composer by model.draft.collectAsStateWithLifecycle()
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

    LaunchedEffect(items.size, items.lastOrNull()?.text, leadingItemCount) {
        if (items.isNotEmpty() && (atBottom || listState.layoutInfo.totalItemsCount == 0)) {
            listState.animateScrollToItem(lastTranscriptIndex)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(state.title.ifEmpty { "Chat" }) },
                navigationIcon = { TextButton(onClick = goBack) { Text("Back") } },
            )
        },
        bottomBar = {
            Composer(
                state = state,
                composer = composer,
                sending = sending,
                cancelling = cancelling,
                model = model,
            )
        },
    ) { padding ->
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
    model: ChatViewModel,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .imePadding()
            .padding(12.dp),
    ) {
        if (state.queue.isNotEmpty()) {
            Row(
                modifier = Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                state.queue.forEachIndexed { index, queued ->
                    AssistChip(
                        onClick = { model.dropQueued(index) },
                        label = { Text(queued, maxLines = 1) },
                    )
                }
            }
        }
        Row(verticalAlignment = Alignment.Bottom) {
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
                    enabled = !cancelling,
                ) {
                    Text(if (cancelling) "Cancelling…" else "Cancel")
                }
            } else {
                Button(
                    onClick = model::send,
                    enabled = !sending && composer.isNotBlank(),
                ) {
                    Text(if (sending) "Sending…" else "Send")
                }
            }
        }
    }
}

@Composable
private fun TranscriptRow(item: TranscriptItem) {
    if (item.kind == TranscriptKind.TOOL) {
        Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = MaterialTheme.shapes.small,
        ) {
            Text(item.text, modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp))
        }
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
                Text(
                    item.text,
                    fontFamily = if (item.kind == TranscriptKind.SYSTEM) {
                        FontFamily.Monospace
                    } else {
                        FontFamily.Default
                    },
                )
            }
        }
    }
}
