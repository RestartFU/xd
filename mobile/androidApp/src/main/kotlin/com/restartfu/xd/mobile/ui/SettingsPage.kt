package com.restartfu.xd.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.annotation.DrawableRes
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.foundation.focusable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R

/** Settings' sections, in the desktop's order and with its names. */
internal enum class SettingsSection(val title: String, val subtitle: String, @DrawableRes val icon: Int) {
    APPEARANCE("Appearance", "How xd looks.", R.drawable.ic_sliders),
    CHATS("Chats", "How chats start, run and are named.", R.drawable.ic_chat),
    AGENTS("Agents", "The assistants xd runs, and what they may do.", R.drawable.ic_terminal),
    CLEANUP("Cleanup", "When finished sessions settle and go away.", R.drawable.ic_trash),
    DEVICES("Devices", "Reach this workspace from other machines.", R.drawable.ic_devices),
    UPDATES("Updates", "Keep xd current.", R.drawable.ic_download),
}

/** The settings page's root, for tests. */
internal const val SETTINGS_PAGE_TAG = "settings-page"

// What the settings drawn as several rows show, shared with search so a word
// on screen always finds its setting.
internal const val CHAT_TITLES_DETAIL = "Name a new chat from its first message."
internal const val NAMING_MODEL_DETAIL = "Also names the worktrees new sessions get."
internal const val QUOTA_PER_CHAT_DETAIL = "A chat's own quota, chosen when it stops, takes the place of this one."
internal const val SETTLE_MERGED_DETAIL =
    "Settle a session when its pull request merges. A closed pull request settles it while either rule here is on."
internal const val SETTLE_INACTIVE_DETAIL =
    "Sessions with no activity for a while settle on their own. Any new activity brings them back."
internal const val SETTLE_DAYS_DETAIL = "How long a session sits without activity before it settles."
internal const val SETTLE_KEEP_PR_DETAIL =
    "Inactivity does not settle sessions with a pull request. Merge and close still do."

/**
 * One block of a section: a setting, or a group of settings the host answers
 * together. [words] are what search matches, besides the section's name.
 */
private class SettingsEntry(
    val section: SettingsSection,
    val words: List<String>,
    val content: @Composable () -> Unit,
)

/** Whether every word of [query] appears in the entry's words or its section's name. */
internal fun settingsMatch(query: String, section: SettingsSection, words: List<String>): Boolean {
    val haystack = (words + section.title).joinToString("\n").lowercase()
    return query.lowercase().split(Regex("\\s+")).filter { it.isNotEmpty() }.all { it in haystack }
}

/**
 * The app's settings as a full-screen page: a searchable list of sections,
 * each opening its own page. Back leaves a section, then clears a search,
 * then closes Settings.
 */
@Composable
internal fun SettingsPage(
    model: MainViewModel,
    settings: MobileSettings,
    onClose: () -> Unit,
    /** Erases this device's pairing with the host, once the reader has confirmed it. */
    onForget: () -> Unit,
    modifier: Modifier = Modifier,
    /** The page's own coming and going, which a back gesture from its home drives. */
    closing: PageTransition<Boolean>? = null,
) {
    val theme by settings.theme.collectAsStateWithLifecycle()
    val allPermissions by settings.allowAllPermissions.collectAsStateWithLifecycle()
    val resumeChats by settings.resumeChats.collectAsStateWithLifecycle()
    val tree by model.client.tree.collectAsStateWithLifecycle()
    // The quota row says its current share, so search reads the same words.
    val quota by model.chatQuota.collectAsStateWithLifecycle()
    // Likewise the settling rules, whose day count shows in a stepper.
    val settleRules by model.autoSettle.collectAsStateWithLifecycle()
    var section by rememberSaveable { mutableStateOf<SettingsSection?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var updatingHost by rememberSaveable { mutableStateOf(false) }
    var accountsOpen by rememberSaveable { mutableStateOf(false) }
    var secretsOpen by rememberSaveable { mutableStateOf(false) }
    var confirmingForget by rememberSaveable { mutableStateOf(false) }

    val entries = buildList {
        add(SettingsEntry(SettingsSection.APPEARANCE, listOf("Theme", "Dark", "Light")) {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Theme", style = MaterialTheme.typography.titleSmall)
                ThemePicker(theme, settings::setTheme)
            }
        })
        add(
            SettingsEntry(
                SettingsSection.CHATS,
                listOf(
                    "Generate chat titles", CHAT_TITLES_DETAIL,
                    "Codex naming model", "Claude naming model", NAMING_MODEL_DETAIL,
                ),
            ) { ChatTitleSettings(model) },
        )
        add(SettingsEntry(SettingsSection.CHATS, listOf("Resume chats after a restart", RESUME_DETAIL)) {
            SettingSwitch(
                title = "Resume chats after a restart",
                detail = RESUME_DETAIL,
                checked = resumeChats,
                onCheckedChange = settings::setResumeChats,
            )
        })
        if (tree.chatQuotas) {
            add(
                SettingsEntry(
                    SettingsSection.CHATS,
                    listOf("Limit each chat's usage", chatQuotaDetail(quota?.percent), "Quota per chat", QUOTA_PER_CHAT_DETAIL),
                ) { ChatQuotaSection(model) },
            )
        }
        add(SettingsEntry(SettingsSection.CHATS, listOf("Prompt shortcuts", SHORTCUTS_DETAIL)) {
            SettingAction(title = "Prompt shortcuts", detail = SHORTCUTS_DETAIL, action = "Edit") {
                model.openShortcutEditor(null, "Prompt shortcuts")
            }
        })
        add(SettingsEntry(SettingsSection.AGENTS, listOf("Assistant accounts", ACCOUNTS_DETAIL, "sign in")) {
            SettingAction(title = "Assistant accounts", detail = ACCOUNTS_DETAIL, action = "Open") { accountsOpen = true }
        })
        add(SettingsEntry(SettingsSection.AGENTS, listOf("Skip permission prompts", PERMISSIONS_DETAIL)) {
            SettingSwitch(
                title = "Skip permission prompts",
                detail = PERMISSIONS_DETAIL,
                checked = allPermissions,
                onCheckedChange = settings::setAllowAllPermissions,
            )
        })
        add(SettingsEntry(SettingsSection.AGENTS, listOf("Agent secrets", SECRETS_DETAIL)) {
            SettingAction(title = "Agent secrets", detail = SECRETS_DETAIL, action = "Edit") { secretsOpen = true }
        })
        if (tree.settlement) {
            add(
                SettingsEntry(
                    SettingsSection.CLEANUP,
                    listOfNotNull(
                        "Settle merged sessions", SETTLE_MERGED_DETAIL,
                        "Settle inactive sessions", SETTLE_INACTIVE_DETAIL,
                        "Days before settling", SETTLE_DAYS_DETAIL,
                        settleRules?.afterDays?.toString(),
                        "Keep sessions with a pull request", SETTLE_KEEP_PR_DETAIL,
                    ),
                ) { AutoSettleSettings(model) },
            )
        }
        // Found under its old name too: it used to be called Disconnect.
        add(SettingsEntry(SettingsSection.DEVICES, listOf(FORGET_HOST_TITLE, FORGET_HOST_DETAIL, "disconnect", "unpair", "remote")) {
            SettingAction(
                title = FORGET_HOST_TITLE,
                detail = FORGET_HOST_DETAIL,
                // The dots say a question follows: nothing is forgotten on this tap.
                action = "Forget…",
                destructive = true,
            ) { confirmingForget = true }
        })
        add(SettingsEntry(SettingsSection.UPDATES, listOf("Host updates", UPDATES_DETAIL)) {
            SettingAction(title = "Host updates", detail = UPDATES_DETAIL, action = "Check") { updatingHost = true }
        })
    }

    // The workspace stays composed beneath the page, so Settings takes the
    // focus and the keyboard from whatever editor or terminal had them.
    val pageFocus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(Unit) {
        pageFocus.requestFocus()
        keyboard?.hide()
    }
    // Search reads the quota's current share, so read it as soon as the host
    // says it has quotas, rather than only once its row is shown.
    LaunchedEffect(tree.chatQuotas) {
        if (tree.chatQuotas) model.loadChatQuota()
    }
    LaunchedEffect(tree.settlement) {
        if (tree.settlement) model.refreshAutoSettle()
    }

    val open = section
    // Back leaves a section, then clears a search, then closes Settings. A
    // back gesture shows the page it returns to as it is dragged: the
    // Settings home from a section, the page beneath from the home.
    val sections = rememberPageTransition(open, "settings-section")
    if (closing != null) {
        PredictiveBack(closing, enabled = open == null && query.isEmpty(), back = false, onBack = onClose)
    } else {
        BackHandler(enabled = open == null && query.isEmpty(), onBack = onClose)
    }
    BackHandler(enabled = open == null && query.isNotEmpty()) { query = "" }
    PredictiveBack(sections, enabled = open != null, back = null) { section = null }

    // A Surface, so taps on the page never reach the workspace beneath it.
    Surface(
        modifier
            .fillMaxSize()
            .testTag(SETTINGS_PAGE_TAG)
            .focusRequester(pageFocus)
            .focusable(),
        color = LocalXdGlass.current.backdrop,
    ) {
    Column(Modifier.fillMaxSize()) {
        SettingsTopBar(
            title = open?.title ?: "Settings",
            // The same order as system Back: leave a section, then a search, then Settings.
            onBack = {
                when {
                    open != null -> section = null
                    query.isNotEmpty() -> query = ""
                    else -> onClose()
                }
            },
        )
        // Into a section and back out along one axis, as stepping into any page.
        val still = reducedMotion()
        sections.transition.AnimatedContent(
            modifier = Modifier.fillMaxSize(),
            transitionSpec = { pageMotion(still, sharedAxisX(forward = targetState != null)) },
        ) { shown ->
        // The page scrolls across its whole width; its rows stop at the
        // reading measure and sit in the middle of a wide window.
        Box(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .navigationBarsPadding(),
            contentAlignment = Alignment.TopCenter,
        ) {
        Column(
            Modifier
                .widthIn(max = ReadingMeasure + 32.dp)
                .fillMaxWidth()
                .padding(horizontal = 16.dp)
                .padding(top = 12.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (shown != null) {
                Text(
                    shown.subtitle,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
                SettingsCard(entries.filter { it.section == shown })
            } else {
                SettingsSearchField(query) { query = it }
                if (query.isBlank()) {
                    val shown = SettingsSection.entries.filter { candidate -> entries.any { it.section == candidate } }
                    SettingsSectionList(
                        sections = shown,
                        summary = { candidate ->
                            when (candidate) {
                                SettingsSection.APPEARANCE -> theme.label
                                SettingsSection.CHATS -> "Resume after a restart: ${if (resumeChats) "On" else "Off"}"
                                SettingsSection.AGENTS -> "Permission prompts: ${if (allPermissions) "Skipped" else "Shown"}"
                                else -> candidate.subtitle
                            }
                        },
                        onOpen = { section = it },
                    )
                } else {
                    val matches = entries.filter { settingsMatch(query, it.section, it.words) }
                    if (matches.isEmpty()) {
                        Text(
                            "No settings match “${query.trim()}”.",
                            modifier = Modifier.padding(top = 8.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    matches.groupBy { it.section }.forEach { (found, group) ->
                        SettingsGroupLabel(found.title)
                        SettingsCard(group)
                    }
                }
            }
        }
        }
        }
    }
    }
    if (updatingHost) {
        HostUpdateDialog(model = model, onDismiss = { updatingHost = false })
    }
    if (accountsOpen) {
        AssistantAccountsDialog(model = model, onDismiss = { accountsOpen = false })
    }
    if (secretsOpen) {
        AgentSecretsDialog(model = model, folderId = null, title = "Agent secrets", onDismiss = { secretsOpen = false })
    }
    if (confirmingForget) {
        ForgetHostDialog(
            onForget = {
                confirmingForget = false
                onForget()
            },
            onDismiss = { confirmingForget = false },
        )
    }
}

@Composable
private fun SettingsTopBar(title: String, onBack: () -> Unit) {
    Column(Modifier.background(glassPane())) {
        Row(
            Modifier
                .fillMaxWidth()
                .safeTopPadding()
                .heightIn(min = 56.dp)
                .padding(start = 4.dp, end = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            IconButton(onClick = onBack) {
                Icon(
                    painterResource(R.drawable.ic_back),
                    contentDescription = "Back",
                    tint = MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier.size(22.dp),
                )
            }
            Text(
                title,
                modifier = Modifier.weight(1f).semantics { heading() },
                style = MaterialTheme.typography.titleLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        HorizontalDivider(color = glassEdge())
        // Settings changes the host too, so it says when the host is out of reach.
        ConnectionLine(divider = glassEdge())
    }
}

@Composable
private fun SettingsSearchField(query: String, onQuery: (String) -> Unit) {
    OutlinedTextField(
        value = query,
        onValueChange = onQuery,
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
        placeholder = { Text("Search settings") },
        leadingIcon = {
            Icon(
                painterResource(R.drawable.ic_search),
                contentDescription = null,
                modifier = Modifier.size(20.dp),
            )
        },
        trailingIcon = if (query.isEmpty()) null else {
            {
                IconButton(onClick = { onQuery("") }) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear search", modifier = Modifier.size(18.dp))
                }
            }
        },
        shape = MaterialTheme.shapes.medium,
        colors = xdFieldColors(focusedContainerColor = glassPane(), unfocusedContainerColor = glassPane()),
    )
}

/** The home page's sections: icon, name and a live summary, each opening its page. */
@Composable
private fun SettingsSectionList(
    sections: List<SettingsSection>,
    summary: (SettingsSection) -> String,
    onOpen: (SettingsSection) -> Unit,
) {
    SettingsCardFrame {
        sections.forEachIndexed { index, item ->
            if (index > 0) HorizontalDivider(color = glassEdge(), modifier = Modifier.padding(start = 60.dp))
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 64.dp)
                    .clickable { onOpen(item) }
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Box(
                    Modifier
                        .size(32.dp)
                        .clip(MaterialTheme.shapes.small)
                        .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.16f)),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        painterResource(item.icon),
                        contentDescription = null,
                        tint = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.size(18.dp),
                    )
                }
                Column(Modifier.weight(1f)) {
                    Text(item.title, style = MaterialTheme.typography.bodyLarge)
                    Text(
                        summary(item),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Icon(
                    painterResource(R.drawable.ic_chevron_right),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(18.dp),
                )
            }
        }
    }
}

@Composable
private fun SettingsGroupLabel(title: String) {
    Text(
        title,
        modifier = Modifier.padding(start = 4.dp, top = 8.dp).semantics { heading() },
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        style = MaterialTheme.typography.labelLarge,
    )
}

/** Entries in one inset glass card, a hairline between each. */
@Composable
private fun SettingsCard(entries: List<SettingsEntry>) {
    SettingsCardFrame {
        entries.forEachIndexed { index, entry ->
            if (index > 0) HorizontalDivider(color = glassEdge())
            Column(
                Modifier
                    .fillMaxWidth()
                    .widthIn(min = 0.dp)
                    .padding(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) { entry.content() }
        }
    }
}

@Composable
private fun SettingsCardFrame(content: @Composable () -> Unit) {
    val shape = MaterialTheme.shapes.medium
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(glassPane())
            .border(1.dp, glassEdge(), shape),
    ) { content() }
    Spacer(Modifier.height(4.dp))
}

private const val RESUME_DETAIL = "Continue turns a restart cut off. Agents start working without asking."
private const val SHORTCUTS_DETAIL = "Buttons above the composer in every project on this host."
private const val ACCOUNTS_DETAIL = "Manage sign-in and the installed assistant CLIs."
private const val PERMISSIONS_DETAIL =
    "New chats pass the agent's unrestricted flag: no approvals or sandbox where the CLI supports it."
private const val SECRETS_DETAIL = "Environment variables the host gives every agent."
internal const val FORGET_HOST_TITLE = "Forget this host"
private const val FORGET_HOST_DETAIL = "Remove this device's pairing with the connected host."
private const val UPDATES_DETAIL = "Check for a new xd build on the connected machine."
