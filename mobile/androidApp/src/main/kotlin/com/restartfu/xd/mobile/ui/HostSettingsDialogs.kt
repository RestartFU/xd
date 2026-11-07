package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.protocol.AgentAuthProviderReply
import com.restartfu.xd.protocol.AgentCliProviderReply
import com.restartfu.xd.protocol.Ops
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Sign-in and the installed assistant CLIs on the connected host, as the
 * desktop's Settings → Assistant accounts shows them. Both belong to the
 * host: signing in here signs the host in.
 */
@Composable
internal fun AssistantAccountsDialog(model: MainViewModel, onDismiss: () -> Unit) {
    val auth by model.agentAuth.collectAsStateWithLifecycle()
    val clis by model.agentClis.collectAsStateWithLifecycle()
    val error by model.accountsError.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { model.loadAccounts() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Assistant accounts") },
        text = {
            Column(
                modifier = Modifier.verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text(
                    "Sign-in and the assistant command-line tools on the connected host.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                val providers = auth?.values?.sortedBy { it.displayName }
                if (providers == null) {
                    CircularProgressIndicator(Modifier.align(Alignment.CenterHorizontally))
                } else {
                    providers.forEach { provider -> AccountRow(provider, model) }
                }
                HorizontalDivider()
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Command-line tools", modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                    TextButton(onClick = model::checkCliUpdates) { Text("Check for updates") }
                }
                clis?.values?.sortedBy { it.displayName }?.forEach { cli -> CliRow(cli, model) }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done") } },
    )
}

@Composable
private fun AccountRow(provider: AgentAuthProviderReply, model: MainViewModel) {
    val uriHandler = LocalUriHandler.current
    val signingIn = provider.state == "signing-in"
    var input by rememberSaveable(provider.provider) { mutableStateOf("") }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = 8.dp)) {
                Text(provider.displayName, fontWeight = FontWeight.SemiBold)
                Text(
                    provider.detail ?: authLabel(provider.state),
                    color = if (provider.state == "failed") MaterialTheme.colorScheme.error
                    else MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (signingIn) {
                TextButton(onClick = { model.cancelSignIn(provider.provider) }) { Text("Cancel sign-in") }
            } else if (provider.state != "signed-in") {
                OutlinedButton(onClick = { model.signIn(provider.provider) }) { Text("Sign in") }
            }
        }
        if (signingIn) {
            provider.deviceCode?.let { code ->
                Text(code, fontFamily = XdMonoFamily, style = MaterialTheme.typography.titleMedium)
            }
            provider.loginUrl?.let { url ->
                TextButton(onClick = { uriHandler.openUri(url) }) { Text("Open sign-in page") }
            }
            if (provider.needsInput) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = input,
                        onValueChange = { input = it },
                        modifier = Modifier.weight(1f),
                        placeholder = { Text("Code from the sign-in page") },
                        singleLine = true,
                    )
                    TextButton(
                        onClick = {
                            model.sendSignInInput(provider.provider, input.trim())
                            input = ""
                        },
                        enabled = input.isNotBlank(),
                    ) { Text("Send") }
                }
            }
        }
    }
}

private fun authLabel(state: String): String = when (state) {
    "signed-in" -> "Signed in"
    "signed-out" -> "Signed out"
    "signing-in" -> "Signing in…"
    "failed" -> "Could not check sign-in"
    else -> "Checking…"
}

@Composable
private fun CliRow(cli: AgentCliProviderReply, model: MainViewModel) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 8.dp)) {
            Text(cli.displayName, fontWeight = FontWeight.SemiBold)
            Text(
                when {
                    cli.version == null -> "Not installed"
                    cli.state == "updating" -> "Updating ${cli.version}…"
                    cli.state == "updated" -> "Updated to ${cli.version}"
                    cli.state == "failed" -> cli.detail ?: "The update failed"
                    cli.available && cli.latest != null -> "${cli.version} · ${cli.latest} available"
                    else -> cli.version.orEmpty()
                },
                color = if (cli.state == "failed") MaterialTheme.colorScheme.error
                else MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (cli.available && cli.updatable && cli.state != "updating") {
            OutlinedButton(onClick = { model.installCliUpdate(cli.provider) }) { Text("Update") }
        }
    }
}

/**
 * Environment secrets the host gives agents: host-wide without a
 * [folderId], otherwise one project's. Values never come back to the phone,
 * so a kept secret is sent by name alone and the host keeps its value.
 */
@Composable
internal fun AgentSecretsDialog(
    model: MainViewModel,
    folderId: String?,
    title: String,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var names by remember { mutableStateOf<List<String>?>(null) }
    var removed by remember { mutableStateOf(emptySet<String>()) }
    var replaced by remember { mutableStateOf(emptyMap<String, String>()) }
    var added by remember { mutableStateOf(emptyList<Pair<String, String>>()) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(attempt) {
        error = null
        try {
            names = model.client.agentSecrets(folderId)
        } catch (failure: CancellationException) {
            throw failure
        } catch (failure: Throwable) {
            error = failure.message ?: "Could not read the secrets"
        }
    }
    val newNames = added.map { it.first.trim() }
    val valid = newNames.all { SECRET_NAME.matches(it) } &&
        added.all { it.second.isNotEmpty() } &&
        (names.orEmpty().filterNot { it in removed } + newNames).let { it.toSet().size == it.size }
    AlertDialog(
        onDismissRequest = { if (!saving) onDismiss() },
        title = { Text(title) },
        text = {
            Column(
                modifier = Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    if (folderId == null) "Given to agents in every project as environment variables."
                    else "Given to this project's agents, over the host-wide ones.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                val current = names
                when {
                    current == null && error == null -> CircularProgressIndicator(Modifier.align(Alignment.CenterHorizontally))
                    current == null -> TextButton(onClick = { attempt++ }) { Text("Try again") }
                    else -> {
                        current.filterNot { it in removed }.forEach { name ->
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(name, modifier = Modifier.weight(0.45f), fontFamily = XdMonoFamily, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                OutlinedTextField(
                                    value = replaced[name].orEmpty(),
                                    onValueChange = { replaced = replaced + (name to it) },
                                    modifier = Modifier.weight(0.55f),
                                    placeholder = { Text("Keep value") },
                                    singleLine = true,
                                    visualTransformation = PasswordVisualTransformation(),
                                )
                                TextButton(onClick = { removed = removed + name }) { Text("Remove") }
                            }
                        }
                        added.forEachIndexed { index, (name, value) ->
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                OutlinedTextField(
                                    value = name,
                                    onValueChange = { text -> added = added.toMutableList().also { it[index] = text to value } },
                                    modifier = Modifier.weight(0.45f),
                                    placeholder = { Text("NAME") },
                                    singleLine = true,
                                    isError = name.isNotBlank() && !SECRET_NAME.matches(name.trim()),
                                )
                                OutlinedTextField(
                                    value = value,
                                    onValueChange = { text -> added = added.toMutableList().also { it[index] = name to text } },
                                    modifier = Modifier.weight(0.55f),
                                    placeholder = { Text("Value") },
                                    singleLine = true,
                                    visualTransformation = PasswordVisualTransformation(),
                                )
                                TextButton(onClick = { added = added.filterIndexed { at, _ -> at != index } }) { Text("Remove") }
                            }
                        }
                        TextButton(onClick = { added = added + ("" to "") }) { Text("Add secret") }
                    }
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    val kept = names.orEmpty().filterNot { it in removed }.map { name ->
                        Ops.SecretEntry(name, replaced[name]?.takeIf(String::isNotEmpty))
                    }
                    val entries = kept + added.map { (name, value) -> Ops.SecretEntry(name.trim(), value) }
                    saving = true
                    error = null
                    scope.launch {
                        try {
                            model.client.setAgentSecrets(folderId, entries)
                            onDismiss()
                        } catch (failure: CancellationException) {
                            throw failure
                        } catch (failure: Throwable) {
                            error = failure.message ?: "Could not save the secrets"
                        } finally {
                            saving = false
                        }
                    }
                },
                enabled = names != null && valid && !saving,
            ) { Text(if (saving) "Saving…" else "Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !saving) { Text("Cancel") } },
    )
}

private val SECRET_NAME = Regex("[A-Za-z_][A-Za-z0-9_]*")

/**
 * Notes the host gives every agent in a project, as the desktop's project
 * context does.
 */
@Composable
internal fun ProjectContextDialog(
    model: MainViewModel,
    folderId: String,
    projectName: String,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var text by rememberSaveable(folderId) { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(folderId, attempt) {
        if (text != null) return@LaunchedEffect
        error = null
        try {
            text = model.client.folderContext(folderId).orEmpty()
        } catch (failure: CancellationException) {
            throw failure
        } catch (failure: Throwable) {
            error = failure.message ?: "Could not read the project context"
        }
    }
    AlertDialog(
        onDismissRequest = { if (!saving) onDismiss() },
        title = { Text("$projectName context") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    "Notes every agent in this project starts with.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                val current = text
                if (current == null) {
                    if (error == null) CircularProgressIndicator(Modifier.align(Alignment.CenterHorizontally))
                    else TextButton(onClick = { attempt++ }) { Text("Try again") }
                } else {
                    OutlinedTextField(
                        value = current,
                        onValueChange = { text = it },
                        modifier = Modifier.fillMaxWidth().heightIn(min = 160.dp, max = 360.dp),
                        enabled = !saving,
                    )
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    saving = true
                    error = null
                    scope.launch {
                        try {
                            model.client.setFolderContext(folderId, text)
                            onDismiss()
                        } catch (failure: CancellationException) {
                            throw failure
                        } catch (failure: Throwable) {
                            error = failure.message ?: "Could not save the project context"
                        } finally {
                            saving = false
                        }
                    }
                },
                enabled = text != null && !saving,
            ) { Text(if (saving) "Saving…" else "Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !saving) { Text("Cancel") } },
    )
}

/** The desktop's Chat titles settings: titling on or off, and each assistant's title model. */
@Composable
internal fun ChatTitleSettings(model: MainViewModel) {
    val settings by model.chatTitles.collectAsStateWithLifecycle()
    val error by model.chatTitlesError.collectAsStateWithLifecycle()
    val catalog by model.hostCatalog.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { model.loadChatTitles() }
    val current = settings
    if (current == null) {
        Text(
            error ?: "Reading chat title settings…",
            color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        if (error != null) TextButton(onClick = model::loadChatTitles) { Text("Try again") }
        return
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text("Generate chat titles", fontWeight = FontWeight.SemiBold)
            Text(
                "Name a new chat from its first message, using a low-cost model.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Switch(checked = current.enabled, onCheckedChange = model::setChatTitlesEnabled)
    }
    if (current.enabled) {
        listOf("codex" to "Codex title model", "claude" to "Claude title model").forEach { (backend, label) ->
            if (current.models.containsKey(backend)) {
                TitleModelRow(
                    label = label,
                    selected = current.models[backend],
                    choices = catalog.firstOrNull { it.id == backend }?.models.orEmpty().map { it.id to it.name },
                    onChoose = { model.setChatTitleModel(backend, it) },
                )
            }
        }
    }
    error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
}

@Composable
private fun TitleModelRow(
    label: String,
    selected: String?,
    choices: List<Pair<String, String>>,
    onChoose: (String) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, modifier = Modifier.weight(1f), fontWeight = FontWeight.SemiBold)
        Box {
            TextButton(onClick = { open = true }, enabled = choices.isNotEmpty()) {
                Text(
                    choices.firstOrNull { it.first == selected }?.second ?: selected ?: "Choose a model",
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                choices.forEach { (id, name) ->
                    DropdownMenuItem(text = { Text(name) }, onClick = { open = false; onChoose(id) })
                }
            }
        }
    }
}
