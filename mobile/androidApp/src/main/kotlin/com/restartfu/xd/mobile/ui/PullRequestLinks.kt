package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.restartfu.xd.model.LinkedPullRequest
import com.restartfu.xd.model.pullRequestNumber

/** Shown when the host refuses what was pasted as not a pull request. */
internal const val NOT_A_PULL_REQUEST_LINK = "That is not a pull request link."

/**
 * The chips a header has room for: all of them up to [limit], otherwise one
 * fewer and a `+N` for the rest, so the count never hides a single chip.
 */
internal fun shownPullRequests(pullRequests: List<LinkedPullRequest>, limit: Int): Pair<List<LinkedPullRequest>, Int> =
    if (pullRequests.size <= limit) {
        pullRequests to 0
    } else {
        val shown = (limit - 1).coerceAtLeast(0)
        pullRequests.take(shown) to pullRequests.size - shown
    }

/** "Merged", "Closed" or "Open" for a known state, otherwise null. */
internal fun pullRequestStateLabel(pullRequest: LinkedPullRequest): String? = when (pullRequest.state) {
    "merged" -> "Merged"
    "closed" -> "Closed"
    "open" -> "Open"
    else -> null
}

/** Open and unknown links read as links; merged and closed ones step back, closed ones struck through. */
@Composable
private fun pullRequestColor(pullRequest: LinkedPullRequest): Color =
    if (pullRequest.merged || pullRequest.closed) LocalXdSignals.current.faint else LocalXdSignals.current.link

private fun pullRequestDecoration(pullRequest: LinkedPullRequest): TextDecoration =
    if (pullRequest.closed) TextDecoration.LineThrough else TextDecoration.Underline

/** What a screen reader says for one linked pull request: its number and how it stands. */
internal fun pullRequestDescription(pullRequest: LinkedPullRequest): String =
    listOfNotNull("Pull request ${pullRequestLabel(pullRequest.url)}", pullRequestStateLabel(pullRequest)?.lowercase())
        .joinToString(", ")

/** "Open pull request #12", the menu entry that opens one. */
internal fun openPullRequestLabel(url: String): String =
    pullRequestNumber(url)?.let { "Open pull request #$it" } ?: "Open pull request"

/**
 * A chat's linked pull requests as `#N` labels, the first few and then `+N`,
 * for a phone header. They are read, not tapped: the row is a line of small
 * text inside the title's own button, with no room for a touch target each,
 * so More opens them.
 */
@Composable
internal fun LinkedPullRequestLabels(
    pullRequests: List<LinkedPullRequest>,
    limit: Int,
    style: TextStyle,
) {
    val (shown, more) = shownPullRequests(pullRequests, limit)
    shown.forEach { pull ->
        key(pull.url) {
            Text(
                pullRequestLabel(pull.url),
                modifier = Modifier.semantics { contentDescription = pullRequestDescription(pull) },
                style = style,
                // Not the link colour: nothing here is tapped.
                color = if (pull.merged || pull.closed) LocalXdSignals.current.faint
                else MaterialTheme.colorScheme.onSurfaceVariant,
                textDecoration = TextDecoration.LineThrough.takeIf { pull.closed },
                maxLines = 1,
            )
        }
    }
    if (more > 0) {
        Text(
            "+$more",
            modifier = Modifier.semantics { contentDescription = "$more more pull requests" },
            style = style,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
        )
    }
}

/**
 * A chat's linked pull requests as buttons with a full touch target each, for
 * a header with room: the first few open their pull request, and `+N` opens
 * the full list.
 */
@Composable
internal fun LinkedPullRequestButtons(
    pullRequests: List<LinkedPullRequest>,
    limit: Int,
    open: (String) -> Unit,
    showAll: () -> Unit,
) {
    val (shown, more) = shownPullRequests(pullRequests, limit)
    shown.forEach { pull ->
        key(pull.url) {
            PullRequestButton(
                label = pullRequestLabel(pull.url),
                description = pullRequestDescription(pull),
                color = pullRequestColor(pull),
                decoration = pullRequestDecoration(pull),
                onClick = { open(pull.url) },
            )
        }
    }
    if (more > 0) {
        PullRequestButton(
            label = "+$more",
            description = "$more more pull requests",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            decoration = null,
            onClick = showAll,
        )
    }
}

/** One pull request link in a header, 48dp square at the least. */
@Composable
internal fun PullRequestButton(
    label: String,
    description: String,
    color: Color,
    decoration: TextDecoration?,
    onClick: () -> Unit,
) {
    TextButton(
        onClick = onClick,
        modifier = Modifier
            .sizeIn(minWidth = 48.dp, minHeight = 48.dp)
            .semantics { contentDescription = description },
        contentPadding = PaddingValues(horizontal = 8.dp),
    ) {
        Text(label, style = MaterialTheme.typography.labelLarge, color = color, textDecoration = decoration, maxLines = 1)
    }
}

/**
 * Every pull request linked to the chat, as the desktop lists them: each
 * opens or can be removed, and pasted links are added. The host rereads the
 * list into the tree after each change, so [pullRequests] is always current.
 */
@Composable
internal fun PullRequestsDialog(
    pullRequests: List<LinkedPullRequest>,
    draft: String,
    onDraft: (String) -> Unit,
    linking: Boolean,
    removing: Set<String>,
    error: String?,
    open: (String) -> Unit,
    link: () -> Unit,
    remove: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val signals = LocalXdSignals.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Pull requests") },
        text = {
            Column(
                modifier = Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (pullRequests.isEmpty()) {
                    Text(
                        "No pull requests linked yet.",
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                } else {
                    Column {
                        pullRequests.forEachIndexed { index, pull ->
                            key(pull.url) {
                                if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Column(
                                        modifier = Modifier
                                            .weight(1f)
                                            .clickable(role = Role.Button, onClickLabel = "Open") { open(pull.url) }
                                            .heightIn(min = 48.dp)
                                            .padding(vertical = 8.dp),
                                    ) {
                                        Row(
                                            verticalAlignment = Alignment.CenterVertically,
                                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                                        ) {
                                            Text(
                                                pullRequestLabel(pull.url),
                                                style = MaterialTheme.typography.titleSmall,
                                                color = pullRequestColor(pull),
                                                textDecoration = pullRequestDecoration(pull),
                                            )
                                            pullRequestStateLabel(pull)?.let {
                                                Text(it, style = MaterialTheme.typography.labelSmall, color = signals.faint)
                                            }
                                        }
                                        Text(
                                            pull.repository ?: pull.url,
                                            style = MaterialTheme.typography.bodySmall,
                                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                                            maxLines = 1,
                                            overflow = TextOverflow.Ellipsis,
                                        )
                                    }
                                    TextButton(onClick = { remove(pull.url) }, enabled = pull.url !in removing) {
                                        Text("Remove")
                                    }
                                }
                            }
                        }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = draft,
                        onValueChange = onDraft,
                        modifier = Modifier.weight(1f),
                        placeholder = { Text("Paste a pull request link") },
                        singleLine = true,
                        enabled = !linking,
                        isError = error != null,
                        colors = xdFieldColors(),
                    )
                    TextButton(onClick = link, enabled = !linking && draft.isNotBlank()) {
                        Text(if (linking) "Linking…" else "Link")
                    }
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}

/** Room for the chips in a chat header: a phone's subtitle row or a tablet's title. */
internal object PullRequestChipLimit {
    const val PHONE = 3
    const val TABLET = 4
}
