package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.LinkedPullRequest

/** The open session's chats, and what can be done to them and to the session. */
internal class SessionChats(
    val chats: List<ChatSummary>,
    val selectedId: String?,
    val select: (String) -> Unit,
    val add: () -> Unit,
    val close: (String) -> Unit,
    val rename: (ChatSummary) -> Unit,
    val delete: (ChatSummary) -> Unit,
    /** Null where the host cannot settle sessions or this one already is. */
    val settleSession: (() -> Unit)? = null,
    val renameSession: () -> Unit = {},
    /** How a chat stands, marked on its row in the chat menu; null for a quiet one. */
    val statusOf: (ChatSummary) -> StatusMark? = { null },
)

/** A delegated agent opened as its own tab inside the chat that started it. */
internal class AgentTab(
    /** The agent's record key. */
    val key: String,
    val title: String,
    val provider: String,
    val running: Boolean,
)

/**
 * The open chat's agent tabs. They are views of the chat's own agents, kept
 * only while the chat is open, never chats of their own.
 */
internal class AgentTabs(
    val tabs: List<AgentTab> = emptyList(),
    /** The tab shown in place of the conversation, if one is. */
    val viewing: String? = null,
    val open: (String) -> Unit = {},
    val close: (String) -> Unit = {},
    val showChat: () -> Unit = {},
)

internal val Pane.icon: Int
    get() = when (this) {
        Pane.CHAT -> R.drawable.ic_chat
        Pane.DIFF -> R.drawable.ic_changes
        Pane.FILES -> R.drawable.ic_folder
        Pane.TERMINAL -> R.drawable.ic_terminal
    }

/**
 * The phone chat's one header row, in place of stacked title, chat-tab and
 * workspace-tab rows, so the conversation keeps the height.
 *
 * The title names the open chat and opens the session's other chats; the
 * named view button swaps the chat for Changes, Files or Terminal; More holds
 * chat settings and the chat and session actions.
 */
@Composable
internal fun CompactChatBar(
    title: String,
    subtitle: String,
    backend: String?,
    pullRequest: String?,
    openPullRequest: (String) -> Unit,
    pane: Pane,
    onPane: (Pane) -> Unit,
    onBack: () -> Unit,
    onChatSettings: () -> Unit,
    chats: SessionChats,
    agents: AgentTabs = AgentTabs(),
    /**
     * The chat's linked pull requests, on a host that keeps them; null on an
     * older one, which shows the branch's [pullRequest] alone.
     */
    linkedPullRequests: List<LinkedPullRequest>? = null,
    /** Opens the Pull requests list; null where the host cannot link them. */
    showPullRequests: (() -> Unit)? = null,
) {
    var choosingChat by remember { mutableStateOf(false) }
    var choosingPane by remember { mutableStateOf(false) }
    var more by remember { mutableStateOf(false) }
    val selected = chats.chats.firstOrNull { it.id == chats.selectedId }
    Column {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .background(glassPane())
                .safeTopPadding()
                // One 56dp row; taller only when enlarged text needs the room.
                .heightIn(min = 56.dp)
                .padding(start = 4.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) {
                Icon(painterResource(R.drawable.ic_back), contentDescription = "Back")
            }
            Box(Modifier.weight(1f)) {
                Column(
                    modifier = Modifier
                        .clip(MaterialTheme.shapes.small)
                        .clickable(onClickLabel = "Switch chat", role = Role.DropdownList) { choosingChat = true }
                        .heightIn(min = 48.dp)
                        .padding(horizontal = 6.dp),
                    verticalArrangement = Arrangement.Center,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (backend != null) BackendIcon(backend, size = 15.dp)
                        Text(
                            title,
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            // Takes the focus as a chat opens, not as its name changes:
                            // a title that arrives mid-message must not leave the composer.
                            modifier = Modifier.weight(1f, fill = false).pageHeading(key = chats.selectedId),
                        )
                        Icon(
                            painterResource(R.drawable.ic_chevron_down), null,
                            modifier = Modifier.size(14.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    // Read here and opened from More: a line of small text
                    // inside the title's button has no room for touch targets.
                    HeaderSubtitle(subtitle, linkedPullRequests, pullRequest)
                }
                DropdownMenu(expanded = choosingChat, onDismissRequest = { choosingChat = false }) {
                    chats.chats.forEach { chat ->
                        val open = chat.id == chats.selectedId && agents.viewing == null
                        // How the chat stands, as the desktop's tabs mark it.
                        val status = chats.statusOf(chat)
                        DropdownMenuItem(
                            text = { Text(chat.title.ifBlank { "New chat" }, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            leadingIcon = { BackendIcon(chat.backend, size = 18.dp) },
                            trailingIcon = if (open || status != null) {
                                {
                                    Row(
                                        verticalAlignment = Alignment.CenterVertically,
                                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                                    ) {
                                        status?.let { StatusMarkIcon(it) }
                                        if (open) Icon(painterResource(R.drawable.ic_check), "Open", Modifier.size(18.dp))
                                    }
                                }
                            } else {
                                null
                            },
                            onClick = { choosingChat = false; agents.showChat(); chats.select(chat.id) },
                        )
                    }
                    // The open chat's agents, so one can be returned to.
                    if (agents.tabs.isNotEmpty()) {
                        HorizontalDivider()
                        agents.tabs.forEach { agent ->
                            DropdownMenuItem(
                                text = { Text(agent.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                leadingIcon = { BackendIcon(agent.provider.lowercase(), size = 18.dp) },
                                trailingIcon = {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        if (agent.running) StatusMarkIcon(StatusMark.WORKING)
                                        if (agent.key == agents.viewing) {
                                            Icon(
                                                painterResource(R.drawable.ic_check), "Open",
                                                Modifier.padding(start = 8.dp).size(18.dp),
                                            )
                                        }
                                        IconButton(onClick = { agents.close(agent.key) }) {
                                            Icon(
                                                painterResource(R.drawable.ic_close), "Close ${agent.title}",
                                                modifier = Modifier.size(16.dp),
                                                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                                            )
                                        }
                                    }
                                },
                                onClick = { choosingChat = false; agents.open(agent.key) },
                            )
                        }
                    }
                    HorizontalDivider()
                    DropdownMenuItem(
                        text = { Text("New chat") },
                        leadingIcon = { Icon(painterResource(R.drawable.ic_add), null, Modifier.size(18.dp)) },
                        onClick = { choosingChat = false; chats.add() },
                    )
                }
            }
            Box {
                // Named, not just drawn: an icon alone does not say that this
                // swaps the whole screen for Changes, Files or Terminal.
                Surface(
                    onClick = { choosingPane = true },
                    shape = MaterialTheme.shapes.small,
                    color = if (pane == Pane.CHAT) Color.Transparent else MaterialTheme.colorScheme.secondaryContainer,
                    border = BorderStroke(1.dp, controlBorder()),
                    modifier = Modifier.semantics { contentDescription = "Workspace view: ${pane.label}" },
                ) {
                    Row(
                        modifier = Modifier.heightIn(min = 32.dp).padding(start = 10.dp, end = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(pane.label, style = MaterialTheme.typography.labelLarge)
                        Icon(
                            painterResource(R.drawable.ic_chevron_down), null,
                            modifier = Modifier.size(14.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                DropdownMenu(expanded = choosingPane, onDismissRequest = { choosingPane = false }) {
                    Pane.entries.forEach { candidate ->
                        DropdownMenuItem(
                            text = { Text(candidate.label) },
                            leadingIcon = { Icon(painterResource(candidate.icon), null, Modifier.size(18.dp)) },
                            trailingIcon = if (candidate == pane) {
                                { Icon(painterResource(R.drawable.ic_check), "Showing", Modifier.size(18.dp)) }
                            } else {
                                null
                            },
                            onClick = { choosingPane = false; onPane(candidate) },
                        )
                    }
                }
            }
            Box {
                IconButton(onClick = { more = true }) {
                    Icon(painterResource(R.drawable.ic_more), contentDescription = "More", modifier = Modifier.size(20.dp))
                }
                DropdownMenu(expanded = more, onDismissRequest = { more = false }) {
                    DropdownMenuItem(
                        text = { Text("Chat settings") },
                        leadingIcon = { Icon(painterResource(R.drawable.ic_settings), null, Modifier.size(18.dp)) },
                        onClick = { more = false; onChatSettings() },
                    )
                    agents.viewing?.let { key ->
                        HorizontalDivider()
                        DropdownMenuItem(text = { Text("Close agent") }, onClick = { more = false; agents.close(key) })
                    }
                    if (selected != null) {
                        HorizontalDivider()
                        DropdownMenuItem(text = { Text("Close chat") }, onClick = { more = false; chats.close(selected.id) })
                        DropdownMenuItem(text = { Text("Rename chat") }, onClick = { more = false; chats.rename(selected) })
                        // Each pull request the header names opens from here.
                        val openable = linkedPullRequests
                            ?.let { shownPullRequests(it, PullRequestChipLimit.PHONE).first.map(LinkedPullRequest::url) }
                            ?: listOfNotNull(pullRequest)
                        openable.forEach { url ->
                            DropdownMenuItem(
                                text = { Text(openPullRequestLabel(url)) },
                                onClick = { more = false; openPullRequest(url) },
                            )
                        }
                        showPullRequests?.let { show ->
                            DropdownMenuItem(text = { Text("Pull requests") }, onClick = { more = false; show() })
                        }
                        DropdownMenuItem(
                            text = { Text("Delete chat…", color = MaterialTheme.colorScheme.error) },
                            onClick = { more = false; chats.delete(selected) },
                        )
                    }
                    HorizontalDivider()
                    chats.settleSession?.let { settle ->
                        DropdownMenuItem(text = { Text("Settle session") }, onClick = { more = false; settle() })
                    }
                    DropdownMenuItem(text = { Text("Rename session") }, onClick = { more = false; chats.renameSession() })
                }
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

/** How the pull requests fit beside the subtitle in the phone header. */
internal enum class SubtitleLinks { ALL, COUNT, NONE }

/**
 * Whether the pull request labels fit beside the subtitle: all of them, or
 * a count in their place, or none. The subtitle names the session and keeps
 * its room; the labels give theirs up first. Widths are in pixels.
 */
internal fun subtitleLinks(available: Int, subtitle: Int, labels: Int, count: Int?, gap: Int): SubtitleLinks = when {
    labels == 0 -> SubtitleLinks.NONE
    subtitle + gap + labels <= available -> SubtitleLinks.ALL
    count != null && subtitle + gap + count <= available -> SubtitleLinks.COUNT
    subtitle == 0 && count != null -> SubtitleLinks.COUNT
    else -> SubtitleLinks.NONE
}

/**
 * The phone header's second line: the session and project, then the chat's
 * pull requests while they fit beside them. Enlarged text gives the line
 * less room; the labels then collapse into a count, then step aside, rather
 * than cut the session's name short. More lists them all either way.
 */
@Composable
private fun HeaderSubtitle(subtitle: String, linkedPullRequests: List<LinkedPullRequest>?, pullRequest: String?) {
    val style = MaterialTheme.typography.labelSmall
    val gap = 8.dp
    val labels = when {
        linkedPullRequests != null -> shownPullRequests(linkedPullRequests, PullRequestChipLimit.PHONE).let { (shown, more) ->
            shown.map { pullRequestLabel(it.url) } + listOfNotNull("+$more".takeIf { more > 0 })
        }
        pullRequest != null -> listOf(pullRequestLabel(pullRequest))
        else -> emptyList()
    }
    val total = linkedPullRequests?.size ?: labels.size
    // A count only stands in for two or more: one label is no wider than its count.
    val countLabel = "$total PRs".takeIf { total > 1 }
    val measurer = rememberTextMeasurer()
    BoxWithConstraints {
        val gapPx = with(LocalDensity.current) { gap.roundToPx() }
        val fit = remember(subtitle, labels, countLabel, style, constraints.maxWidth, gapPx) {
            fun width(text: String) = measurer.measure(text, style, maxLines = 1, softWrap = false).size.width
            subtitleLinks(
                available = constraints.maxWidth,
                subtitle = if (subtitle.isBlank()) 0 else width(subtitle),
                labels = labels.sumOf { width(it) } + gapPx * (labels.size - 1).coerceAtLeast(0),
                count = countLabel?.let(::width),
                gap = gapPx,
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
            if (subtitle.isNotBlank()) Text(
                subtitle,
                modifier = Modifier.weight(1f, fill = false),
                style = style,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            when (fit) {
                SubtitleLinks.ALL -> if (linkedPullRequests != null) {
                    // Every pull request linked to the chat, as the desktop's header lists them.
                    LinkedPullRequestLabels(
                        pullRequests = linkedPullRequests,
                        limit = PullRequestChipLimit.PHONE,
                        style = style,
                    )
                } else {
                    // The branch's pull request, as the desktop's session header shows it.
                    pullRequest?.let { url ->
                        Text(
                            pullRequestLabel(url),
                            modifier = Modifier.semantics { contentDescription = "Pull request ${pullRequestLabel(url)}" },
                            style = style,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                SubtitleLinks.COUNT -> Text(
                    countLabel.orEmpty(),
                    modifier = Modifier.semantics { contentDescription = "$total pull requests" },
                    style = style,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                )
                SubtitleLinks.NONE -> Unit
            }
        }
    }
}
