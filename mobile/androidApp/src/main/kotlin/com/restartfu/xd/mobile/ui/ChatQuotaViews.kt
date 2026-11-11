package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.model.ChatQuota

/** What the host-wide chat quota does, with the desktop's wording. */
internal fun chatQuotaDetail(percent: Int?): String =
    "Limit each chat to ${percent?.let { "$it%" } ?: "a share"} of its account's 5-hour usage limit. " +
        "A chat that reaches it stops and asks before going on."

/** The quota card's headline, with the desktop's wording. */
internal const val QUOTA_REACHED_TITLE = "This chat reached its quota"

/** What the chat used and what its quota was, with the desktop's wording. */
internal fun quotaReachedSentence(quota: ChatQuota?): String =
    if (quota == null) "It used its share of its account's 5-hour limit."
    else "It used ${quota.usedLabel}% of its account's 5-hour limit; its quota was ${quota.percent}%."

/**
 * The host's default chat quota: off, or a whole percentage of each chat's
 * account's usage limit. Shown only on hosts whose `tree` has `chat_quotas`.
 */
@Composable
internal fun ChatQuotaSection(model: MainViewModel) {
    val settings by model.chatQuota.collectAsStateWithLifecycle()
    val error by model.chatQuotaError.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { model.loadChatQuota() }
    val current = settings
    if (current == null) {
        Text(
            error ?: "Reading the chat quota…",
            color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        if (error != null) TextButton(onClick = model::loadChatQuota) { Text("Try again") }
        return
    }
    val percent = current.percent
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text("Chat quota", fontWeight = FontWeight.SemiBold)
            Text(
                chatQuotaDetail(percent),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Switch(
            checked = percent != null,
            onCheckedChange = { on -> model.setChatQuota(if (on) ChatQuota.DEFAULT_PERCENT else null) },
        )
    }
    if (percent != null) {
        // Keyed by the host's value, so a change made elsewhere replaces what is shown.
        var typed by rememberSaveable(percent) { mutableStateOf(percent.toString()) }
        val chosen = ChatQuota.typedPercent(typed)
        val commit = { if (chosen != null && chosen != percent) model.setChatQuota(chosen) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = 12.dp)) {
                Text("Quota per chat", fontWeight = FontWeight.SemiBold)
                Text(
                    "A chat's own quota, chosen when it stops, takes the place of this one.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            PercentField(
                text = typed,
                onText = { typed = it },
                onDone = commit,
                modifier = Modifier.onFocusChanged { if (!it.isFocused) commit() },
                label = "Quota per chat",
            )
        }
    }
    error?.let {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(it, modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            TextButton(onClick = model::loadChatQuota) { Text("Try again") }
        }
    }
}

/**
 * Shown in place of the resume button while the chat is stopped at its
 * quota: continue with a new quota, twice the last one to begin with, or
 * without a limit. Messages sent meanwhile wait in the queue.
 */
@Composable
internal fun QuotaReachedCard(
    quota: ChatQuota?,
    busy: Boolean,
    error: String?,
    onContinue: (percent: Int?) -> Unit,
    modifier: Modifier = Modifier,
) {
    var typed by rememberSaveable(quota?.percent) { mutableStateOf(ChatQuota.nextPercent(quota?.percent).toString()) }
    val chosen = ChatQuota.typedPercent(typed)
    // Waiting on the user like a question, so it wears the same mark.
    val waiting = LocalXdSignals.current.waiting
    Surface(
        modifier = modifier.fillMaxWidth(),
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(1.dp, waiting.copy(alpha = 0.6f)),
    ) {
        Column(
            Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(QUOTA_REACHED_TITLE, modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
                StatusPill("Waiting on you", waiting)
            }
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    quotaReachedSentence(quota),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                Text(
                    "Messages you send wait in the queue until it goes on.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("New quota", style = MaterialTheme.typography.bodyMedium)
                PercentField(
                    text = typed,
                    onText = { typed = it },
                    onDone = { if (chosen != null && !busy) onContinue(chosen) },
                    enabled = !busy,
                    label = "New quota",
                )
            }
            if (chosen == null) {
                Text(
                    "Enter a whole number from 1 to 100.",
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilledTonalButton(onClick = { chosen?.let(onContinue) }, enabled = chosen != null && !busy) {
                    Text("Continue")
                }
                TextButton(onClick = { onContinue(null) }, enabled = !busy) { Text("Continue without a limit") }
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
        }
    }
}

/** A whole percentage from 1 to 100, typed with the number keyboard. */
@Composable
private fun PercentField(
    text: String,
    onText: (String) -> Unit,
    onDone: () -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    OutlinedTextField(
        value = text,
        onValueChange = { typed -> onText(typed.filter(Char::isDigit).take(3)) },
        // The visible words beside it are not tied to it, so screen readers
        // are told what the number is.
        modifier = modifier.width(92.dp).semantics { contentDescription = "$label, percent" },
        enabled = enabled,
        singleLine = true,
        isError = ChatQuota.typedPercent(text) == null,
        suffix = { Text("%") },
        textStyle = MaterialTheme.typography.bodyMedium,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
    )
}
