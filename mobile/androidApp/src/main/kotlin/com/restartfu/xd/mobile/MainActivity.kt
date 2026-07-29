package com.restartfu.xd.mobile

import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link

class MainActivity : ComponentActivity() {
    private val model: MainViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(Modifier.fillMaxSize()) {
                    XdMobileApp(model)
                }
            }
        }
    }
}

@Composable
private fun XdMobileApp(model: MainViewModel) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val hasCredentials by model.client.hasCredentials.collectAsStateWithLifecycle()
    val link by model.client.link.collectAsStateWithLifecycle()

    DisposableEffect(lifecycleOwner, model.client) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> model.client.poke()
                Lifecycle.Event.ON_STOP -> model.client.goBackground()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    if (!hasCredentials) {
        PairScreen(model)
    } else if (link is Link.Fatal) {
        FatalScreen(link as Link.Fatal, model::forget)
    } else {
        ConnectedNavigation(model, link)
    }
}

@Composable
private fun PairScreen(model: MainViewModel) {
    var host by rememberSaveable { mutableStateOf("") }
    var port by rememberSaveable { mutableStateOf("4001") }
    var deviceName by rememberSaveable { mutableStateOf(Build.MODEL.orEmpty()) }
    var code by rememberSaveable { mutableStateOf("") }
    val pairing by model.pairing.collectAsStateWithLifecycle()
    val error by model.error.collectAsStateWithLifecycle()
    val valid = host.isNotBlank() &&
        port.toIntOrNull() in 1..65535 &&
        deviceName.isNotBlank() &&
        code.length == 9

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(24.dp)
            .imePadding(),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("Pair with xd", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.height(8.dp))
        Text("Run xd serve --pair on the machine, then enter its address and code.")
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = host,
            onValueChange = { host = it.trim() },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Host or Tailscale IP") },
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = port,
            onValueChange = { port = it.filter(Char::isDigit).take(5) },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Port") },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = deviceName,
            onValueChange = { deviceName = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Device name") },
            singleLine = true,
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = code,
            onValueChange = { code = formatPairingCode(it) },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Pairing code") },
            supportingText = { Text("ABCDEFGHJKLMNPQRSTUVWXYZ23456789") },
            singleLine = true,
        )
        error?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(20.dp))
        Button(
            onClick = {
                model.pair(host, port.toInt(), code, deviceName)
            },
            enabled = valid && !pairing,
            modifier = Modifier.fillMaxWidth(),
        ) {
            if (pairing) {
                CircularProgressIndicator(
                    modifier = Modifier.width(20.dp),
                    strokeWidth = 2.dp,
                )
            } else {
                Text("Pair")
            }
        }
    }
}

@Composable
private fun FatalScreen(
    fatal: Link.Fatal,
    forget: () -> Unit,
) {
    val pinMismatch = fatal.reason == FatalReason.PIN_MISMATCH
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text(
            if (pinMismatch) "Machine identity changed" else "Connection refused",
            style = MaterialTheme.typography.headlineMedium,
        )
        Spacer(Modifier.height(12.dp))
        Text(
            if (pinMismatch) {
                "This is not the machine you paired with: its certificate changed. " +
                    "Re-pair only if you changed or reinstalled the daemon yourself."
            } else {
                fatal.message
            },
        )
        Spacer(Modifier.height(24.dp))
        Button(onClick = forget) { Text("Forget and re-pair") }
    }
}

@Composable
private fun ConnectedNavigation(
    model: MainViewModel,
    link: Link,
) {
    val navigation = rememberNavController()
    NavHost(navigation, startDestination = "tree") {
        composable("tree") {
            TreeScreen(
                model = model,
                link = link,
                openChat = { navigation.navigate("chat/${Uri.encode(it)}") },
            )
        }
        composable(
            route = "chat/{chatId}",
            arguments = listOf(navArgument("chatId") { type = NavType.StringType }),
        ) { entry ->
            val chatId = entry.arguments?.getString("chatId").orEmpty()
            val chatModel: ChatViewModel = viewModel(
                key = chatId,
                factory = ChatViewModel.Factory(model.client, chatId),
            )
            ChatScreen(chatModel, navigation::popBackStack)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TreeScreen(
    model: MainViewModel,
    link: Link,
    openChat: (String) -> Unit,
) {
    val tree by model.client.tree.collectAsStateWithLifecycle()
    val operationError by model.error.collectAsStateWithLifecycle()
    val expanded = remember { mutableStateMapOf<String, Boolean>() }
    val roots = tree.folders.filter { it.parentId == null }
    val children = tree.folders.groupBy(Folder::parentId)
    val chats = tree.chats.groupBy(ChatSummary::folderId)
    val foldersById = tree.folders.associateBy(Folder::id)
    var choosingFolder by rememberSaveable { mutableStateOf(false) }
    var confirmingForget by rememberSaveable { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("xd") },
                actions = {
                    TextButton(onClick = { confirmingForget = true }) {
                        Text("Forget")
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton(
                onClick = { choosingFolder = true },
            ) { Text("New") }
        },
    ) { padding ->
        Column(Modifier.padding(padding)) {
            ConnectionBanner(link, model.client::poke)
            if (tree.loading && tree.folders.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
            } else {
                (tree.error ?: operationError)?.let {
                    Text(
                        it,
                        modifier = Modifier.padding(16.dp),
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                LazyColumn(contentPadding = PaddingValues(bottom = 88.dp)) {
                    roots.forEach { folder ->
                        folderRows(folder, 0, children, chats, expanded, openChat)
                    }
                }
            }
        }
    }
    if (choosingFolder) {
        AlertDialog(
            onDismissRequest = { choosingFolder = false },
            title = { Text("Choose a folder") },
            text = {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState()),
                ) {
                    if (tree.folders.isEmpty()) {
                        Text("No workspace folders are available.")
                    }
                    tree.folders.forEach { folder ->
                        Text(
                            folderPath(folder, foldersById),
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    choosingFolder = false
                                    model.createChat(folder.id, openChat)
                                }
                                .padding(vertical = 12.dp),
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { choosingFolder = false }) { Text("Cancel") }
            },
        )
    }
    if (confirmingForget) {
        AlertDialog(
            onDismissRequest = { confirmingForget = false },
            title = { Text("Forget this remote?") },
            text = {
                Text("Pairing credentials will be erased. You will need a new code to reconnect.")
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmingForget = false
                        model.forget()
                    },
                ) { Text("Forget") }
            },
            dismissButton = {
                TextButton(onClick = { confirmingForget = false }) { Text("Cancel") }
            },
        )
    }
}

private fun folderPath(
    folder: Folder,
    foldersById: Map<String, Folder>,
): String {
    val names = mutableListOf<String>()
    val visited = mutableSetOf<String>()
    var current: Folder? = folder
    while (current != null && visited.add(current.id)) {
        names += current.name
        current = current.parentId?.let(foldersById::get)
    }
    return names.asReversed().joinToString(" / ")
}

private fun androidx.compose.foundation.lazy.LazyListScope.folderRows(
    folder: Folder,
    depth: Int,
    children: Map<String?, List<Folder>>,
    chats: Map<String, List<ChatSummary>>,
    expanded: MutableMap<String, Boolean>,
    openChat: (String) -> Unit,
) {
    item(key = "folder-${folder.id}") {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable { expanded[folder.id] = expanded[folder.id] != true }
                .padding(start = (16 + depth * 16).dp, top = 12.dp, bottom = 12.dp),
        ) {
            Text(if (expanded[folder.id] == true) "▾" else "▸")
            Spacer(Modifier.width(8.dp))
            Text(folder.name, style = MaterialTheme.typography.titleMedium)
        }
    }
    if (expanded[folder.id] == true) {
        chats[folder.id].orEmpty().forEach { chat ->
            item(key = "chat-${chat.id}") {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { openChat(chat.id) }
                        .padding(
                            start = (40 + depth * 16).dp,
                            top = 10.dp,
                            bottom = 10.dp,
                            end = 16.dp,
                        ),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(chat.title, modifier = Modifier.weight(1f))
                    if (chat.working) Text("working", color = MaterialTheme.colorScheme.primary)
                }
            }
        }
        children[folder.id].orEmpty().forEach { child ->
            folderRows(child, depth + 1, children, chats, expanded, openChat)
        }
    }
}

@Composable
private fun ConnectionBanner(
    link: Link,
    retry: () -> Unit,
) {
    when (link) {
        Link.Connecting -> Text(
            "Connecting…",
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
        )
        is Link.Down -> Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Offline — retrying in ${link.nextAttemptInMs / 1000}s",
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = retry) { Text("Retry") }
        }
        else -> Unit
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ChatScreen(
    model: ChatViewModel,
    goBack: () -> Unit,
) {
    val state by model.state.collectAsStateWithLifecycle()
    val sending by model.sending.collectAsStateWithLifecycle()
    var composer by rememberSaveable { mutableStateOf("") }
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
                        onValueChange = { composer = it },
                        modifier = Modifier.weight(1f),
                        label = { Text("Message") },
                        minLines = 1,
                        maxLines = 5,
                    )
                    Spacer(Modifier.width(8.dp))
                    Button(
                        onClick = {
                            if (state.working) {
                                model.cancel()
                            } else {
                                val text = composer
                                model.send(text) {
                                    if (composer == text) composer = ""
                                }
                            }
                        },
                        enabled = state.working || (!sending && composer.isNotBlank()),
                    ) {
                        Text(
                            when {
                                state.working -> "Cancel"
                                sending -> "Sending…"
                                else -> "Send"
                            },
                        )
                    }
                }
            }
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

@Composable
private fun TranscriptRow(item: TranscriptItem) {
    if (item.kind == TranscriptKind.TOOL) {
        AssistChip(onClick = {}, label = { Text(item.text) })
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

private fun formatPairingCode(input: String): String {
    val raw = input.uppercase().filter { it in PAIRING_ALPHABET }.take(8)
    return if (raw.length > 4) raw.take(4) + "-" + raw.drop(4) else raw
}

private const val PAIRING_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
