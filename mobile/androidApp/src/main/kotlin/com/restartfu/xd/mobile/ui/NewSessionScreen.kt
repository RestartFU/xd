package com.restartfu.xd.mobile.ui

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.model.ProjectChoice
import com.restartfu.xd.model.matching
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.BranchSourceReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.PullRequestSourceReply
import com.restartfu.xd.protocol.WorktreeReply
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import org.json.JSONObject

/** Everything a new session or chat starts with. */
internal data class NewSessionRequest(
    /** Blank leaves the name to the host. */
    val title: String,
    val agent: DirectAgent,
    /** Where it works; null for a chat joining a session, or the host's own default. */
    val worktree: Ops.NewSessionWorktree?,
    val startFromRemote: Boolean,
    val effort: String?,
    val access: String?,
    val plan: Boolean,
)

/**
 * New session, from the Projects page: one full-screen surface that first
 * asks which project, then what to start there. [projectId] null shows the
 * list; choosing one moves on, and Change or Back goes back to it.
 */
@Composable
internal fun NewSessionFlow(
    choices: List<ProjectChoice>,
    lastUsed: String?,
    icons: Map<String, ByteArray?>,
    projectId: String?,
    /** Back from the form returns to the list rather than closing. */
    returnsToList: Boolean,
    /** A project to start in, or null to go back to the list. */
    onChoose: (String?) -> Unit,
    backends: List<BackendReply>,
    catalogError: String?,
    retryCatalog: () -> Unit,
    places: suspend (projectId: String) -> NewSessionPlaces,
    onDismiss: () -> Unit,
    onCreate: (projectId: String, NewSessionRequest) -> Unit,
) {
    FullScreenSheet(onDismiss) {
        // Forward into the form, back out to the list, as a stack of pages moves.
        AnimatedContent(
            targetState = projectId,
            transitionSpec = { pageTransition(forward = targetState != null) },
            label = "new-session-step",
        ) { id ->
            val chosen = choices.firstOrNull { it.project.id == id }
            if (id == null || chosen == null) {
                ProjectStep(
                    choices = choices,
                    lastUsed = lastUsed,
                    icons = icons,
                    onClose = onDismiss,
                    onPick = { onChoose(it) },
                )
            } else key(id) {
                SessionForm(
                    header = FormHeader(chosen.project.name, chosen.path.joinToString(" / ").ifEmpty { null }, icons[chosen.project.owner]),
                    draftKey = "$id/new",
                    backends = backends,
                    catalogError = catalogError,
                    retryCatalog = retryCatalog,
                    chatTab = false,
                    places = { places(id) },
                    navigation = if (returnsToList) Navigation.Back { onChoose(null) } else Navigation.Close(onDismiss),
                    onChangeProject = { onChoose(null) }.takeIf { choices.size > 1 },
                    onCreate = { onCreate(id, it) },
                )
            }
        }
    }
}

/**
 * The new session form on its own, for a project already chosen, or with
 * [chatTab] for a chat joining an existing session.
 */
@Composable
internal fun NewSessionDialog(
    projectName: String,
    draftKey: String? = null,
    backends: List<BackendReply> = emptyList(),
    catalogError: String? = null,
    retryCatalog: (() -> Unit)? = null,
    chatTab: Boolean = false,
    /** Where a new session can work; null for a chat joining an existing session. */
    places: (suspend () -> NewSessionPlaces)? = null,
    onDismiss: () -> Unit,
    onCreate: (NewSessionRequest) -> Unit,
) {
    FullScreenSheet(onDismiss) {
        SessionForm(
            header = FormHeader(
                name = projectName,
                detail = if (chatTab) "Shares this session's workspace" else null,
                icon = null,
                session = chatTab,
            ),
            draftKey = draftKey,
            backends = backends,
            catalogError = catalogError,
            retryCatalog = retryCatalog,
            chatTab = chatTab,
            places = places,
            navigation = Navigation.Close(onDismiss),
            onChangeProject = null,
            onCreate = onCreate,
        )
    }
}

private sealed interface Navigation {
    val action: () -> Unit

    class Close(override val action: () -> Unit) : Navigation
    class Back(override val action: () -> Unit) : Navigation
}

private data class FormHeader(
    val name: String,
    val detail: String?,
    val icon: ByteArray?,
    /** A session a chat joins, rather than a project. */
    val session: Boolean = false,
)

private fun pageTransition(forward: Boolean) = sharedAxisX(forward)

/** A whole-screen surface over the app, rising from the bottom as a full-screen dialog does. */
@Composable
private fun FullScreenSheet(onDismiss: () -> Unit, content: @Composable () -> Unit) {
    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        val window = (LocalView.current.parent as? DialogWindowProvider)?.window
        LaunchedEffect(window) { window?.setWindowAnimations(android.R.style.Animation_InputMethod) }
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) { content() }
    }
}

@Composable
private fun TopBar(title: String, navigation: Navigation) {
    Row(
        Modifier
            .fillMaxWidth()
            .statusBarsPadding()
            .height(64.dp)
            .padding(start = 4.dp, end = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        IconButton(onClick = navigation.action) {
            Icon(
                painterResource(if (navigation is Navigation.Back) R.drawable.ic_back else R.drawable.ic_close),
                contentDescription = if (navigation is Navigation.Back) "Back" else "Close",
                modifier = Modifier.size(20.dp),
            )
        }
        Text(
            title,
            modifier = Modifier.semantics { heading() },
            style = MaterialTheme.typography.titleLarge,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** More projects than this, and the list offers a filter. */
private const val PROJECT_FILTER_AT = 6

/** The first step: which project the session starts in. */
@Composable
private fun ProjectStep(
    choices: List<ProjectChoice>,
    lastUsed: String?,
    icons: Map<String, ByteArray?>,
    onClose: () -> Unit,
    onPick: (String) -> Unit,
) {
    var filter by rememberSaveable { mutableStateOf("") }
    val shown = choices.matching(filter)
    Column(Modifier.fillMaxSize()) {
        TopBar("New session", Navigation.Close(onClose))
        LazyColumn(
            // Ends above the keyboard and navigation bar, so the last rows can scroll into view.
            modifier = Modifier.weight(1f).windowInsetsPadding(WindowInsets.navigationBars.union(WindowInsets.ime)),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
        ) {
            item("question") {
                Text(
                    "Which project?",
                    modifier = Modifier.padding(start = 4.dp, bottom = 16.dp),
                    style = MaterialTheme.typography.headlineSmall,
                )
            }
            // Kept while it filters, so a list that shrinks cannot hide projects behind it.
            if (choices.size > PROJECT_FILTER_AT || filter.isNotEmpty()) {
                item("filter") {
                    OutlinedTextField(
                        value = filter,
                        onValueChange = { filter = it },
                        label = { Text("Find a project") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                        modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp),
                    )
                }
            }
            if (shown.isEmpty()) {
                item("none") {
                    Text(
                        "No project matches “${filter.trim()}”",
                        modifier = Modifier.padding(start = 4.dp, top = 8.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
            if (shown.isNotEmpty()) item("projects") {
                Group {
                    shown.forEachIndexed { index, choice ->
                        key(choice.project.id) {
                            if (index > 0) HorizontalDivider(Modifier.padding(start = 56.dp), color = MaterialTheme.colorScheme.outlineVariant)
                            ProjectChoiceRow(
                                choice = choice,
                                icon = icons[choice.project.owner],
                                lastUsed = choice.project.id == lastUsed && choices.size > 1,
                                onClick = { onPick(choice.project.id) },
                            )
                        }
                    }
                }
            }
        }
    }
}


@Composable
private fun ProjectChoiceRow(
    choice: ProjectChoice,
    icon: ByteArray?,
    lastUsed: Boolean,
    onClick: () -> Unit,
) {
    val faint = LocalXdSignals.current.faint
    val project = choice.project
    // Where it sits, and how busy it is, so two projects of one name tell apart.
    val detail = listOfNotNull(
        choice.path.joinToString(" / ").ifEmpty { null },
        if (project.sessions == 0) "No sessions" else "${project.sessions} session${if (project.sessions == 1) "" else "s"}",
    ).joinToString(" · ")
    Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 68.dp)
                .clickable(onClick = onClick)
                .padding(horizontal = 16.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            OwnerAvatar(icon = icon, fallback = R.drawable.ic_folder, tint = MaterialTheme.colorScheme.onSurfaceVariant, size = 24.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    project.name,
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    detail,
                    style = MaterialTheme.typography.bodySmall,
                    color = faint,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (lastUsed) StatusPill("Last used", MaterialTheme.colorScheme.onSurfaceVariant)
            Icon(
                painterResource(R.drawable.ic_chevron_right),
                contentDescription = null,
                tint = faint,
                modifier = Modifier.size(16.dp),
            )
    }
}

private enum class FormPage { FORM, BASE }

/** The form: where, what, with which assistant, and how it may work. */
@Composable
private fun SessionForm(
    header: FormHeader,
    draftKey: String?,
    backends: List<BackendReply>,
    catalogError: String?,
    retryCatalog: (() -> Unit)?,
    chatTab: Boolean,
    places: (suspend () -> NewSessionPlaces)?,
    navigation: Navigation,
    onChangeProject: (() -> Unit)?,
    onCreate: (NewSessionRequest) -> Unit,
) {
    val context = LocalContext.current
    val drafts = remember(context) { context.getSharedPreferences("new-chat-drafts", Context.MODE_PRIVATE) }
    val restored = remember(draftKey) {
        runCatching { JSONObject(draftKey?.let { drafts.getString(it, null) } ?: "{}") }.getOrDefault(JSONObject())
    }
    var title by rememberSaveable(draftKey) { mutableStateOf(restored.optString("title")) }
    var agent by rememberSaveable(draftKey) {
        mutableStateOf(DirectAgent.entries.firstOrNull { it.wire == restored.optString("agent") } ?: DirectAgent.CODEX)
    }
    var effort by rememberSaveable(draftKey) { mutableStateOf(restored.optString("effort").takeIf(String::isNotBlank)) }
    var access by rememberSaveable(draftKey) { mutableStateOf(restored.optString("access").takeIf(String::isNotBlank)) }
    var plan by rememberSaveable(draftKey) { mutableStateOf(restored.optBoolean("plan")) }
    var startFromRemote by rememberSaveable(draftKey) { mutableStateOf(restored.optBoolean("startFromRemote", true)) }
    var loaded by remember { mutableStateOf<NewSessionPlaces?>(null) }
    var placesError by remember { mutableStateOf<String?>(null) }
    var worktree by rememberSaveable(draftKey, stateSaver = WorktreeChoiceSaver) {
        mutableStateOf(runCatching { restored.optJSONObject("worktree")?.let(::worktreeFromJson) }.getOrNull())
    }
    var page by rememberSaveable(draftKey) { mutableStateOf(FormPage.FORM) }
    val saveDraft = {
        draftKey?.let { key ->
            drafts.edit().putString(
                key,
                JSONObject().put("title", title).put("agent", agent.wire)
                    .put("effort", effort).put("access", access).put("plan", plan).put("startFromRemote", startFromRemote)
                    .put("worktree", worktree?.let(::worktreeToJson)).toString(),
            ).apply()
        }
    }
    LaunchedEffect(draftKey, title, agent, effort, access, plan, worktree, startFromRemote) { saveDraft() }
    if (places != null) {
        LaunchedEffect(Unit) {
            try {
                val found = places()
                loaded = found
                worktree = found.reconcile(worktree)
            } catch (error: kotlinx.coroutines.CancellationException) {
                throw error
            } catch (error: Throwable) {
                // The host still picks the project's own checkout, so a choice
                // restored from before a rotation is dropped rather than sent unseen.
                worktree = null
                placesError = error.message ?: "Could not list where a session can work"
            }
        }
    }
    if (navigation is Navigation.Back) BackHandler(onBack = navigation.action)

    val efforts = backends.firstOrNull { it.id == agent.wire }?.efforts.orEmpty()
    val catalogMissing = retryCatalog != null && backends.isEmpty()
    // Wait for the choices, as the desktop does; if they cannot be read, the
    // host's own default checkout still works. A project with no checkout to
    // list leaves the choice to the host.
    val waitingOnPlaces = places != null && placesError == null &&
        (loaded == null || (worktree == null && loaded?.default != null))
    val blocker = when {
        catalogMissing && catalogError != null -> "Assistant options could not load"
        catalogMissing -> "Loading assistant options…"
        places != null && placesError == null && loaded == null -> "Reading the project's checkouts…"
        waitingOnPlaces -> "Choose where it works"
        else -> null
    }
    val busy = blocker != null && blocker.endsWith("…")
    val create = {
        // Saved as sent, so the draft is let go once the session is made
        // rather than kept for an edit the effect had not saved yet.
        saveDraft()
        onCreate(
            NewSessionRequest(
                title = title.trim(),
                agent = agent,
                worktree = worktree,
                startFromRemote = startFromRemote,
                effort = effort?.takeIf { it in efforts },
                access = access,
                plan = plan,
            ),
        )
    }

    AnimatedContent(
        targetState = page,
        transitionSpec = { pageTransition(forward = targetState == FormPage.BASE) },
        label = "new-session-page",
    ) { shown ->
        when (shown) {
            FormPage.BASE -> BasePicker(
                places = loaded,
                selected = worktree,
                onBack = { page = FormPage.FORM },
                onPick = {
                    worktree = it
                    page = FormPage.FORM
                },
            )
            FormPage.FORM -> Column(Modifier.fillMaxSize()) {
                TopBar(if (chatTab) "New chat" else "New session", navigation)
                Column(
                    Modifier
                        .weight(1f)
                        .verticalScroll(rememberScrollState())
                        .padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(28.dp),
                ) {
                    HeaderCard(header, onChangeProject)
                    OutlinedTextField(
                        value = title,
                        onValueChange = { title = it },
                        label = { Text("What are you working on?") },
                        supportingText = { Text("Optional. You can rename it later.") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(
                            capitalization = KeyboardCapitalization.Sentences,
                            imeAction = ImeAction.Done,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FormSection("Assistant") {
                        AgentTiles(agent) { agent = it; effort = null }
                        // Loading is said once, above Start; a failure is said here, with its fix.
                        if (catalogMissing && catalogError != null) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    catalogError,
                                    modifier = Modifier.weight(1f).padding(start = 4.dp),
                                    color = MaterialTheme.colorScheme.error,
                                    style = MaterialTheme.typography.bodySmall,
                                )
                                retryCatalog?.let { TextButton(onClick = it) { Text("Try again") } }
                            }
                        }
                    }
                    if (places != null) {
                        FormSection("Workspace") {
                            WorkspaceSection(
                                places = loaded,
                                error = placesError,
                                selected = worktree,
                                startFromRemote = startFromRemote,
                                onSelect = { worktree = it },
                                onStartFromRemote = { startFromRemote = it },
                                onChooseBase = { page = FormPage.BASE },
                            )
                        }
                    }
                    FormSection("Options") {
                        OptionsGroup(
                            efforts = efforts,
                            effort = effort,
                            onEffort = { effort = it },
                            access = access,
                            onAccess = { access = it },
                            plan = plan,
                            onPlan = { plan = it },
                        )
                    }
                }
                ActionBar(
                    label = if (chatTab) "Start chat" else "Start session",
                    blocker = blocker,
                    busy = busy,
                    onClick = create,
                )
            }
        }
    }
}

/**
 * The chosen segment gets a tonal fill that shows in both themes, beside
 * Material's check, so the choice never rests on colour alone.
 */
@Composable
private fun segmentedColors() = SegmentedButtonDefaults.colors(
    activeContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
    activeContentColor = MaterialTheme.colorScheme.onSurface,
    activeBorderColor = MaterialTheme.colorScheme.outline,
    inactiveContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    inactiveContentColor = MaterialTheme.colorScheme.onSurfaceVariant,
    inactiveBorderColor = MaterialTheme.colorScheme.outline,
)

@Composable
private fun FormSection(label: String, content: @Composable ColumnScope.() -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(
            label,
            modifier = Modifier.padding(start = 4.dp).semantics { heading() },
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        content()
    }
}

/** A grouped surface for rows that belong together. */
@Composable
private fun Group(content: @Composable ColumnScope.() -> Unit) {
    val shape = MaterialTheme.shapes.medium
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape),
        content = content,
    )
}

@Composable
private fun GroupDivider() {
    HorizontalDivider(Modifier.padding(start = 16.dp), color = MaterialTheme.colorScheme.outlineVariant)
}

/** Where the session starts: the project, with a way back to the others. */
@Composable
private fun HeaderCard(header: FormHeader, onChange: (() -> Unit)?) {
    Group {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 68.dp)
                .then(if (onChange != null) Modifier.clickable(onClickLabel = "Choose another project", onClick = onChange) else Modifier)
                .padding(start = 16.dp, end = if (onChange != null) 8.dp else 16.dp, top = 10.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            OwnerAvatar(
                icon = header.icon,
                fallback = if (header.session) R.drawable.ic_sessions else R.drawable.ic_folder,
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                size = 24.dp,
            )
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    header.name,
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                header.detail?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = LocalXdSignals.current.faint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            if (onChange != null) TextButton(onClick = onChange) { Text("Change") }
        }
    }
}

/** One tile per assistant; the chosen one is filled, outlined, and checked. */
@Composable
private fun AgentTiles(selected: DirectAgent, onSelect: (DirectAgent) -> Unit) {
    Row(Modifier.fillMaxWidth().selectableGroup(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        DirectAgent.entries.forEach { candidate ->
            val chosen = candidate == selected
            val shape = MaterialTheme.shapes.medium
            Box(
                Modifier
                    .weight(1f)
                    .height(84.dp)
                    .clip(shape)
                    .background(if (chosen) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerLow)
                    .border(
                        if (chosen) 1.5.dp else 1.dp,
                        if (chosen) MaterialTheme.colorScheme.outline else MaterialTheme.colorScheme.outlineVariant,
                        shape,
                    )
                    .selectable(selected = chosen, role = Role.RadioButton, onClick = { onSelect(candidate) }),
            ) {
                Column(
                    Modifier.align(Alignment.Center),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    BackendIcon(candidate.wire, size = 22.dp)
                    Text(
                        candidate.label,
                        style = MaterialTheme.typography.labelLarge,
                        color = if (chosen) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (chosen) {
                    Icon(
                        painterResource(R.drawable.ic_check),
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.onSurface,
                        modifier = Modifier.align(Alignment.TopEnd).padding(8.dp).size(14.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun Waiting(text: String) {
    Row(
        Modifier.padding(start = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
    }
}

/**
 * The project's own checkout, or a new worktree: a fresh branch, or one made
 * from a branch or pull request, as the desktop offers.
 */
@Composable
private fun WorkspaceSection(
    places: NewSessionPlaces?,
    error: String?,
    selected: Ops.NewSessionWorktree?,
    startFromRemote: Boolean,
    onSelect: (Ops.NewSessionWorktree) -> Unit,
    onStartFromRemote: (Boolean) -> Unit,
    onChooseBase: () -> Unit,
) {
    when {
        error != null -> Text(
            "$error The project's own checkout will be used.",
            modifier = Modifier.padding(start = 4.dp),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        places == null -> Group {
            Box(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 16.dp), contentAlignment = Alignment.CenterStart) {
                Waiting("Reading the project's checkouts…")
            }
        }
        else -> {
            val newWorktree = selected != null && selected !is Ops.NewSessionWorktree.Existing
            val canBranch = places.branches.isNotEmpty() || places.pullRequests.isNotEmpty() ||
                selected is Ops.NewSessionWorktree.Branch || selected is Ops.NewSessionWorktree.PullRequest
            val canNew = places.canCreateWorktree || canBranch
            if (canNew && places.worktrees.isNotEmpty()) {
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    SegmentedButton(
                        selected = !newWorktree,
                        // Only a switch picks the default; the checkout already chosen stays.
                        onClick = { if (newWorktree) places.default?.let(onSelect) },
                        shape = SegmentedButtonDefaults.itemShape(0, 2),
                        colors = segmentedColors(),
                    ) { Text("Current worktree", maxLines = 1) }
                    SegmentedButton(
                        selected = newWorktree,
                        onClick = {
                            when {
                                newWorktree -> Unit
                                places.canCreateWorktree -> onSelect(Ops.NewSessionWorktree.New)
                                else -> onChooseBase()
                            }
                        },
                        shape = SegmentedButtonDefaults.itemShape(1, 2),
                        colors = segmentedColors(),
                    ) { Text("New worktree", maxLines = 1) }
                }
                Text(
                    if (newWorktree) "Create an isolated branch and checkout" else "Use the project's existing checkout",
                    modifier = Modifier.padding(start = 4.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (!newWorktree && places.worktrees.isNotEmpty()) {
                Group {
                    places.worktrees.forEachIndexed { index, checkout ->
                        if (index > 0) GroupDivider()
                        ChoiceRow(
                            label = checkout.branch ?: checkout.path.substringAfterLast('/'),
                            detail = if (checkout.main) "Project directory" else checkout.path,
                            chosen = selected == Ops.NewSessionWorktree.Existing(checkout.path),
                            mono = checkout.branch != null,
                            onClick = { onSelect(Ops.NewSessionWorktree.Existing(checkout.path)) },
                        )
                    }
                }
            } else if (places.canCreateWorktree || canBranch) {
                Group {
                    var rows = 0
                    if (places.canCreateWorktree) {
                        rows++
                        ChoiceRow(
                            label = "Fresh branch",
                            detail = "In its own checkout",
                            chosen = selected == Ops.NewSessionWorktree.New,
                            onClick = { onSelect(Ops.NewSessionWorktree.New) },
                        )
                    }
                    if (canBranch) {
                        if (rows++ > 0) GroupDivider()
                        BaseRow(selected, places, onChooseBase)
                    }
                    if (selected is Ops.NewSessionWorktree.Branch) {
                        if (rows > 0) GroupDivider()
                        SwitchRow(
                            title = "Start from remote",
                            detail = "Use the latest remote branch when available",
                            checked = startFromRemote,
                            onCheckedChange = onStartFromRemote,
                        )
                    }
                }
            }
            places.sourcesError?.let {
                Text(
                    "Branches and pull requests could not be listed: $it",
                    modifier = Modifier.padding(start = 4.dp),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            // Said here too: with nothing listed, the branch page cannot be opened to read it.
            places.pullRequestError?.let {
                Text(
                    it,
                    modifier = Modifier.padding(start = 4.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

/** The branch or pull request a new worktree starts from, chosen on its own page. */
@Composable
private fun BaseRow(selected: Ops.NewSessionWorktree?, places: NewSessionPlaces, onChoose: () -> Unit) {
    val (value, mono) = when (selected) {
        is Ops.NewSessionWorktree.Branch -> branchName(selected.reference) to true
        is Ops.NewSessionWorktree.PullRequest ->
            (places.pullRequests.firstOrNull { it.number == selected.number }?.let { "#${it.number} ${it.title}" }
                ?: "#${selected.number}") to false
        else -> null to false
    }
    val chosen = value != null
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .clickable(onClick = onChoose)
            .padding(start = 4.dp, end = 16.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = chosen, onClick = null, modifier = Modifier.size(48.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                value ?: "From a branch or pull request",
                style = MaterialTheme.typography.bodyLarge,
                fontFamily = if (mono) XdMonoFamily else null,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (chosen) "Change branch or pull request" else "Check it out in its own worktree",
                style = MaterialTheme.typography.bodySmall,
                color = LocalXdSignals.current.faint,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Icon(
            painterResource(R.drawable.ic_chevron_right),
            contentDescription = null,
            tint = LocalXdSignals.current.faint,
            modifier = Modifier.size(16.dp),
        )
    }
}

private fun branchName(reference: String) = reference.removePrefix("refs/heads/").removePrefix("refs/remotes/")

@Composable
private fun ChoiceRow(label: String, detail: String, chosen: Boolean, onClick: () -> Unit, mono: Boolean = false) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .selectable(selected = chosen, role = Role.RadioButton, onClick = onClick)
            .padding(start = 4.dp, end = 16.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = chosen, onClick = null, modifier = Modifier.size(48.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                label,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                fontFamily = if (mono) XdMonoFamily else null,
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                detail,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable
private fun SwitchRow(title: String, detail: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = LocalXdSignals.current.faint)
        }
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}

private val ACCESS = listOf(null to "Default", "read-only" to "Read only", "edit" to "Edit", "full" to "Full access")

/** How the agent may work: its effort, what it may touch, and whether it plans first. */
@Composable
private fun OptionsGroup(
    efforts: List<String>,
    effort: String?,
    onEffort: (String?) -> Unit,
    access: String?,
    onAccess: (String?) -> Unit,
    plan: Boolean,
    onPlan: (Boolean) -> Unit,
) {
    Group {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 64.dp)
                .padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("Mode", style = MaterialTheme.typography.bodyLarge)
                Text(
                    if (plan) "Plans before editing" else "Edits as it works",
                    style = MaterialTheme.typography.bodySmall,
                    color = LocalXdSignals.current.faint,
                )
            }
            SingleChoiceSegmentedButtonRow(Modifier.width(200.dp)) {
                SegmentedButton(
                    selected = !plan,
                    onClick = { onPlan(false) },
                    shape = SegmentedButtonDefaults.itemShape(0, 2),
                    colors = segmentedColors(),
                ) { Text("Build", maxLines = 1) }
                SegmentedButton(
                    selected = plan,
                    onClick = { onPlan(true) },
                    shape = SegmentedButtonDefaults.itemShape(1, 2),
                    colors = segmentedColors(),
                ) { Text("Plan", maxLines = 1) }
            }
        }
        GroupDivider()
        ValueRow(
            label = "Access",
            detail = if (plan) "Plan mode only reads" else null,
            value = if (plan) "Read only" else ACCESS.firstOrNull { it.first == access }?.second ?: "Default",
            choices = ACCESS.map { it.second },
            enabled = !plan,
            onSelect = { label -> onAccess(ACCESS.firstOrNull { it.second == label }?.first) },
        )
        if (efforts.isNotEmpty()) {
            GroupDivider()
            ValueRow(
                label = "Reasoning effort",
                detail = null,
                value = effort?.let(::effortLabel) ?: "Default",
                choices = listOf("Default") + efforts.map(::effortLabel),
                onSelect = { label -> onEffort(efforts.firstOrNull { effortLabel(it) == label }) },
            )
        }
    }
}

private fun effortLabel(effort: String): String = when (effort.lowercase()) {
    "xhigh" -> "Extra high"
    else -> effort.replaceFirstChar { it.uppercase() }
}

/** A setting with a few named values, chosen from a menu at the row's end. */
@Composable
private fun ValueRow(
    label: String,
    detail: String?,
    value: String,
    choices: List<String>,
    enabled: Boolean = true,
    onSelect: (String) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val tone = if (enabled) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurface.copy(alpha = 0.5f)
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
            .clickable(enabled = enabled, onClickLabel = "Change $label", role = Role.DropdownList) { open = true }
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.bodyLarge, color = tone)
            detail?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = LocalXdSignals.current.faint) }
        }
        Box {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(value, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Icon(
                    painterResource(R.drawable.ic_chevron_down),
                    contentDescription = null,
                    tint = LocalXdSignals.current.faint,
                    modifier = Modifier.size(14.dp),
                )
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                choices.forEach { choice ->
                    DropdownMenuItem(
                        text = { Text(choice) },
                        trailingIcon = if (choice == value) {
                            { Icon(painterResource(R.drawable.ic_check), contentDescription = "Chosen", modifier = Modifier.size(16.dp)) }
                        } else null,
                        onClick = { open = false; onSelect(choice) },
                    )
                }
            }
        }
    }
}

/** The one action, always in reach above the keyboard, saying what it waits for. */
@Composable
private fun ActionBar(label: String, blocker: String?, busy: Boolean, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface)) {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Column(
            Modifier
                .windowInsetsPadding(WindowInsets.navigationBars.union(WindowInsets.ime))
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (blocker != null) {
                if (busy) {
                    Waiting(blocker)
                } else {
                    Text(
                        blocker,
                        modifier = Modifier.padding(start = 4.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            Button(
                onClick = onClick,
                enabled = blocker == null,
                shape = MaterialTheme.shapes.medium,
                modifier = Modifier.fillMaxWidth().height(52.dp),
            ) { Text(label, style = MaterialTheme.typography.labelLarge) }
        }
    }
}

/** Branches and pull requests shown at once; the filter finds the rest. */
private const val PLACE_LIMIT = 12

/**
 * The first [limit] of [matches], plus the chosen one from [all] even when
 * the filter or the limit leaves it out, and how many matches are not shown.
 * A chosen row shown outside the filter does not stand in for a hidden match.
 */
internal fun <T> limitPlaces(
    matches: List<T>,
    all: List<T>,
    limit: Int = PLACE_LIMIT,
    chosen: (T) -> Boolean,
): Pair<List<T>, Int> {
    val first = matches.take(limit)
    val shown = first + all.filter { chosen(it) && it !in first }
    return shown to matches.count { it !in shown }
}

/** Picks the branch or pull request a new worktree checks out. */
@Composable
private fun BasePicker(
    places: NewSessionPlaces?,
    selected: Ops.NewSessionWorktree?,
    onBack: () -> Unit,
    onPick: (Ops.NewSessionWorktree) -> Unit,
) {
    BackHandler(onBack = onBack)
    var filter by rememberSaveable { mutableStateOf("") }
    val query = filter.trim()
    val pulls = places?.pullRequests.orEmpty()
        .filter { query.isEmpty() || it.title.contains(query, true) || "#${it.number}".contains(query) }
    val branches = places?.branches.orEmpty().filter { query.isEmpty() || it.name.contains(query, true) }
    // The chosen one always shows, whatever the filter and limit hide.
    val chosenPull = (selected as? Ops.NewSessionWorktree.PullRequest)?.number
    val chosenBranch = (selected as? Ops.NewSessionWorktree.Branch)?.reference
    val (shownPulls, hiddenPulls) = limitPlaces(pulls, places?.pullRequests.orEmpty()) { it.number == chosenPull }
    val (shownBranches, hiddenBranches) = limitPlaces(branches, places?.branches.orEmpty()) { it.reference == chosenBranch }
    Column(Modifier.fillMaxSize()) {
        TopBar("Branch or pull request", Navigation.Back(onBack))
        LazyColumn(
            // Ends above the keyboard and navigation bar, so the last rows can scroll into view.
            modifier = Modifier.weight(1f).windowInsetsPadding(WindowInsets.navigationBars.union(WindowInsets.ime)),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
        ) {
            item("filter") {
                OutlinedTextField(
                    value = filter,
                    onValueChange = { filter = it },
                    label = { Text("Find a branch or pull request") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                    modifier = Modifier.fillMaxWidth().padding(bottom = 20.dp),
                )
            }
            placeGroup(
                key = "pulls",
                title = "Pull requests",
                rows = shownPulls,
                hidden = hiddenPulls,
                label = { "#${it.number} ${it.title}" },
                detail = { "Pull request" },
                chosen = { selected == Ops.NewSessionWorktree.PullRequest(it.number, it.oid) },
                mono = false,
                onPick = { onPick(Ops.NewSessionWorktree.PullRequest(it.number, it.oid)) },
            )
            placeGroup(
                key = "branches",
                title = "Branches",
                rows = shownBranches,
                hidden = hiddenBranches,
                label = BranchSourceReply::name,
                detail = { if (it.reference.startsWith("refs/remotes/")) "Remote branch" else "Branch" },
                chosen = { selected == Ops.NewSessionWorktree.Branch(it.reference, it.oid) },
                mono = true,
                onPick = { onPick(Ops.NewSessionWorktree.Branch(it.reference, it.oid)) },
            )
            // A choice kept through a failed listing still shows, so Start
            // never uses something the page does not show.
            val kept = when (selected) {
                is Ops.NewSessionWorktree.PullRequest -> if (pulls.none { it.number == selected.number } && places?.pullRequests.orEmpty().none { it.number == selected.number }) {
                    "#${selected.number}" to "Pull request, chosen before it could be listed"
                } else null
                is Ops.NewSessionWorktree.Branch -> if (places?.branches.orEmpty().none { it.reference == selected.reference }) {
                    branchName(selected.reference) to "Branch, chosen before it could be listed"
                } else null
                else -> null
            }
            kept?.let { (label, detail) ->
                item("kept") {
                    Group { ChoiceRow(label = label, detail = detail, chosen = true, onClick = onBack, mono = selected is Ops.NewSessionWorktree.Branch) }
                }
            }
            if (shownPulls.isEmpty() && shownBranches.isEmpty() && kept == null) {
                item("none") {
                    Text(
                        if (query.isEmpty()) "This project has no branches or pull requests to start from"
                        else "Nothing matches “$query”",
                        modifier = Modifier.padding(start = 4.dp, top = 8.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
            places?.pullRequestError?.let { message ->
                item("pull-error") {
                    Text(
                        message,
                        modifier = Modifier.padding(start = 4.dp, top = 12.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }
}

private fun <T> androidx.compose.foundation.lazy.LazyListScope.placeGroup(
    key: String,
    title: String,
    rows: List<T>,
    /** Matches the limit leaves out; a chosen row shown outside the filter is not one. */
    hidden: Int,
    label: (T) -> String,
    detail: (T) -> String,
    chosen: (T) -> Boolean,
    mono: Boolean,
    onPick: (T) -> Unit,
) {
    if (rows.isEmpty()) return
    item("$key-title") {
        Text(
            title,
            modifier = Modifier.padding(start = 4.dp, bottom = 12.dp).semantics { heading() },
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    item("$key-rows") {
        Group {
            rows.forEachIndexed { index, row ->
                if (index > 0) GroupDivider()
                ChoiceRow(label = label(row), detail = detail(row), chosen = chosen(row), mono = mono, onClick = { onPick(row) })
            }
        }
    }
    item("$key-more") {
        if (hidden > 0) {
            Text(
                "$hidden more. Search to find them.",
                modifier = Modifier.padding(start = 4.dp, top = 8.dp),
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Spacer(Modifier.height(24.dp))
    }
}

/** Where a new session in a project can work, as the desktop's new session dialog offers. */
internal data class NewSessionPlaces(
    val worktrees: List<WorktreeReply>,
    val canCreateWorktree: Boolean,
    val branches: List<BranchSourceReply>,
    val pullRequests: List<PullRequestSourceReply>,
    val pullRequestError: String?,
    /** Why branches and pull requests could not be listed at all. */
    val sourcesError: String? = null,
) {
    /**
     * A choice restored from before a rotation, matched against what the host
     * lists now: a branch or pull request keeps its identity with its current
     * commit, and anything no longer offered falls back to the default.
     */
    fun reconcile(choice: Ops.NewSessionWorktree?): Ops.NewSessionWorktree? = when (choice) {
        null -> default
        Ops.NewSessionWorktree.New -> if (canCreateWorktree) choice else default
        is Ops.NewSessionWorktree.Existing -> if (worktrees.any { it.path == choice.path }) choice else default
        // A failed listing says nothing about whether the choice still exists,
        // so it is kept, and the host checks its commit again on creating.
        is Ops.NewSessionWorktree.Branch -> if (sourcesError != null) choice else
            branches.firstOrNull { it.reference == choice.reference }
                ?.let { Ops.NewSessionWorktree.Branch(it.reference, it.oid) } ?: default
        // Pull requests can fail to list on their own, while branches still list.
        is Ops.NewSessionWorktree.PullRequest -> if (sourcesError != null || pullRequestError != null) choice else
            pullRequests.firstOrNull { it.number == choice.number }
                ?.let { Ops.NewSessionWorktree.PullRequest(it.number, it.oid) } ?: default
    }

    /** The checkout in use, else the project's own, as the desktop picks. */
    val default: Ops.NewSessionWorktree?
        get() = (worktrees.firstOrNull { it.current } ?: worktrees.firstOrNull { it.main } ?: worktrees.firstOrNull())
            ?.let { Ops.NewSessionWorktree.Existing(it.path) }

    companion object {
        suspend fun load(client: com.restartfu.xd.XdClient, folderId: String): NewSessionPlaces = coroutineScope {
            val settings = async { client.folderSettings(folderId) }
            // Branches and pull requests are extras: a project without Git, or a
            // host without them, still offers its checkouts.
            val sources = async { runCatching { client.newChatSources(folderId) } }
            val folder = settings.await()
            val listed = sources.await()
            val found = listed.getOrNull()
            val worktrees = folder.worktrees.ifEmpty {
                listOfNotNull(folder.effectiveWorkdir?.takeIf(String::isNotBlank)?.let {
                    WorktreeReply(path = it, detached = false, main = true, current = true)
                })
            }
            NewSessionPlaces(
                worktrees = worktrees,
                canCreateWorktree = folder.canCreateWorktree,
                branches = found?.branches.orEmpty(),
                pullRequests = found?.pullRequests.orEmpty(),
                pullRequestError = found?.pullRequestError,
                sourcesError = listed.exceptionOrNull()?.let { it.message ?: "Could not list branches and pull requests" },
            )
        }
    }
}

private fun worktreeFromJson(saved: JSONObject): Ops.NewSessionWorktree? = when (saved.optString("kind")) {
    "new" -> Ops.NewSessionWorktree.New
    "existing" -> Ops.NewSessionWorktree.Existing(saved.getString("path"))
    "branch" -> Ops.NewSessionWorktree.Branch(saved.getString("ref"), saved.getString("oid"))
    "pull-request" -> Ops.NewSessionWorktree.PullRequest(saved.getLong("number"), saved.getString("oid"))
    else -> null
}

private fun worktreeToJson(choice: Ops.NewSessionWorktree): JSONObject = JSONObject().apply {
    when (choice) {
        Ops.NewSessionWorktree.New -> put("kind", "new")
        is Ops.NewSessionWorktree.Existing -> { put("kind", "existing"); put("path", choice.path) }
        is Ops.NewSessionWorktree.Branch -> { put("kind", "branch"); put("ref", choice.reference); put("oid", choice.oid) }
        is Ops.NewSessionWorktree.PullRequest -> { put("kind", "pull-request"); put("number", choice.number); put("oid", choice.oid) }
    }
}

/** Keeps a workspace choice across rotation. */
private val WorktreeChoiceSaver = listSaver<Ops.NewSessionWorktree?, String>(
    save = { choice ->
        when (choice) {
            null -> emptyList()
            Ops.NewSessionWorktree.New -> listOf("new")
            is Ops.NewSessionWorktree.Existing -> listOf("existing", choice.path)
            is Ops.NewSessionWorktree.Branch -> listOf("branch", choice.reference, choice.oid)
            is Ops.NewSessionWorktree.PullRequest -> listOf("pull-request", choice.number.toString(), choice.oid)
        }
    },
    restore = { saved ->
        when (saved.firstOrNull()) {
            "new" -> Ops.NewSessionWorktree.New
            "existing" -> Ops.NewSessionWorktree.Existing(saved[1])
            "branch" -> Ops.NewSessionWorktree.Branch(saved[1], saved[2])
            "pull-request" -> Ops.NewSessionWorktree.PullRequest(saved[1].toLong(), saved[2])
            else -> null
        }
    },
)
