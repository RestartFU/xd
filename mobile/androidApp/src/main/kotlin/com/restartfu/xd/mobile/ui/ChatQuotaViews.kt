package com.restartfu.xd.mobile.ui

import androidx.compose.ui.semantics.Role
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Button
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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

/** What follows the used figure in the quota ask, with the desktop's wording. */
internal fun quotaUsedSentence(quota: ChatQuota): String =
    "used of the 5-hour limit, past its ${quota.percent}% quota"

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
            modifier = Modifier.announced(),
            color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        if (error != null) TextButton(onClick = model::loadChatQuota) { Text("Try again") }
        return
    }
    val percent = current.percent
    // One stop, named by its words: a tap anywhere on the row turns it.
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .toggleable(
                value = percent != null,
                role = Role.Switch,
                onValueChange = { on -> model.setChatQuota(if (on) ChatQuota.DEFAULT_PERCENT else null) },
            ),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text("Limit each chat's usage", fontWeight = FontWeight.SemiBold)
            Text(
                chatQuotaDetail(percent),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Switch(checked = percent != null, onCheckedChange = null)
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
                    QUOTA_PER_CHAT_DETAIL,
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
            Text(it, modifier = Modifier.weight(1f).announced(), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            TextButton(onClick = model::loadChatQuota) { Text("Try again") }
        }
    }
}

/**
 * Docked in the composer in place of the resume button while the chat is
 * stopped at its quota: continue with a new quota, twice the last one to
 * begin with, or without a limit. Messages sent meanwhile wait in the queue,
 * which the composer's placeholder says.
 */
@Composable
@OptIn(ExperimentalLayoutApi::class)
internal fun QuotaAsk(
    quota: ChatQuota?,
    busy: Boolean,
    error: String?,
    onContinue: (percent: Int?) -> Unit,
) {
    var typed by rememberSaveable(quota?.percent) { mutableStateOf(ChatQuota.nextPercent(quota?.percent).toString()) }
    val chosen = ChatQuota.typedPercent(typed)
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    val small = MaterialTheme.typography.bodyMedium
    AskFrame {
        AskHeader(QUOTA_REACHED_TITLE)
        if (quota != null) {
            Row(Modifier.padding(horizontal = AskInset), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                Text(
                    "${quota.usedLabel}%",
                    modifier = Modifier.alignByBaseline(),
                    style = MaterialTheme.typography.titleLarge.copy(
                        fontWeight = FontWeight.SemiBold,
                        fontFeatureSettings = "tnum",
                    ),
                )
                Text(quotaUsedSentence(quota), modifier = Modifier.alignByBaseline(), color = muted, style = small)
            }
        } else {
            Text(quotaReachedSentence(null), modifier = Modifier.padding(horizontal = AskInset), color = muted, style = small)
        }
        FlowRow(
            modifier = Modifier.fillMaxWidth().padding(horizontal = AskInset),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("New quota", modifier = Modifier.align(Alignment.CenterVertically), color = muted, style = small)
            QuotaPercentField(
                text = typed,
                onText = { typed = it },
                onDone = { if (chosen != null && !busy) onContinue(chosen) },
                enabled = !busy,
                label = "New quota",
                modifier = Modifier.align(Alignment.CenterVertically),
            )
            Button(
                onClick = { chosen?.let(onContinue) },
                enabled = chosen != null && !busy,
                contentPadding = AskButtonPadding,
                modifier = Modifier.align(Alignment.CenterVertically),
            ) { Text("Continue") }
            Spacer(Modifier.weight(1f))
            TextButton(
                onClick = { onContinue(null) },
                enabled = !busy,
                modifier = Modifier.align(Alignment.CenterVertically),
            ) { Text("Continue without a limit", color = if (busy) LocalXdSignals.current.faint else muted) }
        }
        if (chosen == null) {
            Text(
                "Enter a whole number from 1 to 100.",
                modifier = Modifier.padding(horizontal = AskInset).announced(),
                color = MaterialTheme.colorScheme.error,
                style = small,
            )
        }
        error?.let {
            Text(it, modifier = Modifier.padding(horizontal = AskInset).announced(), color = MaterialTheme.colorScheme.error, style = small)
        }
    }
}

/**
 * The quota field in the ask: small, with `%` inside, its border the danger
 * colour while what is typed is not a quota. Drawn at 40dp; the platform
 * still gives it a 48dp touch target.
 */
@Composable
private fun QuotaPercentField(
    text: String,
    onText: (String) -> Unit,
    onDone: () -> Unit,
    label: String,
    enabled: Boolean,
    modifier: Modifier = Modifier,
) {
    val valid = ChatQuota.typedPercent(text) != null
    BasicTextField(
        value = text,
        onValueChange = { typed -> onText(typed.filter(Char::isDigit).take(3)) },
        modifier = modifier
            .minimumInteractiveComponentSize()
            .width(76.dp)
            // At least 40dp: enlarged text needs more.
            .heightIn(min = 40.dp)
            .semantics { contentDescription = "$label, percent" },
        enabled = enabled,
        singleLine = true,
        textStyle = MaterialTheme.typography.bodyMedium.copy(
            color = MaterialTheme.colorScheme.onSurface,
            fontFeatureSettings = "tnum",
        ),
        cursorBrush = SolidColor(MaterialTheme.colorScheme.onSurface),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        decorationBox = { field ->
            Row(
                modifier = Modifier
                    .clip(MaterialTheme.shapes.small)
                    .background(MaterialTheme.colorScheme.surfaceContainerLow)
                    .border(
                        1.dp,
                        if (valid) controlBorder() else MaterialTheme.colorScheme.error,
                        MaterialTheme.shapes.small,
                    )
                    .padding(horizontal = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.weight(1f)) { field() }
                Text("%", color = LocalXdSignals.current.faint, style = MaterialTheme.typography.bodyMedium)
            }
        },
    )
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
        colors = xdFieldColors(),
    )
}
