package com.restartfu.xd.mobile.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.R
import com.restartfu.xd.mobile.SignInDraft
import com.restartfu.xd.mobile.signInAttempt
import com.restartfu.xd.protocol.AgentAccountReply
import com.restartfu.xd.protocol.AgentAuthProviderReply
import kotlin.math.roundToInt
import kotlinx.coroutines.delay

/** What the dialog says above the accounts, as the desktop's Settings, Agents does. */
internal const val ACCOUNTS_INTRO =
    "Each assistant can hold several sign-ins. New chats start on the first one with room left; " +
        "a chat stays on its account until that account runs out. " +
        "To keep a chat on one account, choose it in the chat's settings."

/** The assistants that can hold several accounts, in the order shown; any other follows. */
private val ACCOUNT_PROVIDERS = listOf("codex", "claude")

internal fun accountProviderOrder(providers: Collection<AgentAuthProviderReply>): List<AgentAuthProviderReply> =
    providers.sortedWith(
        compareBy<AgentAuthProviderReply> { ACCOUNT_PROVIDERS.indexOf(it.provider).let { index -> if (index < 0) Int.MAX_VALUE else index } }
            .thenBy { it.displayName },
    )

/** How an account row's status reads: plain, a hint, a problem, or the account in use. */
internal enum class AccountTone { MUTED, FAINT, DANGER, WORKING }

internal data class AccountStatus(val text: String, val tone: AccountTone, val dot: Boolean = false)

/** "2 h 5 min", "12 min", "3 d". */
internal fun timeUntil(atMs: Long, nowMs: Long): String {
    val minutes = ((atMs - nowMs).coerceAtLeast(0) + 59_999) / 60_000
    val days = minutes / (24 * 60)
    val hours = minutes / 60 % 24
    val rest = minutes % 60
    return when {
        days == 0L && hours == 0L -> "${rest.coerceAtLeast(1)} min"
        days == 0L && rest == 0L -> "$hours h"
        days == 0L -> "$hours h $rest min"
        hours == 0L -> "$days d"
        else -> "$days d $hours h"
    }
}

/** What an account row says about the account, with the desktop's wording. */
internal fun accountStatus(account: AgentAccountReply, nowMs: Long): AccountStatus {
    when (account.state) {
        "signing-in" -> return AccountStatus(account.detail ?: "Signing in…", AccountTone.MUTED)
        "signing-out" -> return AccountStatus(account.detail ?: "Signing out…", AccountTone.MUTED)
        "signed-out" -> return AccountStatus("Signed out", AccountTone.MUTED)
        "failed" -> return AccountStatus(account.detail ?: "Sign-in failed", AccountTone.DANGER)
    }
    when (account.problem) {
        null -> Unit
        "mismatch" -> return AccountStatus("Now signed in to a different account, so it is not used", AccountTone.DANGER)
        else -> return AccountStatus("Same login as an account above, so it is not used", AccountTone.DANGER)
    }
    account.limitedUntilMs?.takeIf { it > nowMs }?.let { until ->
        val reason = account.limitedReason ?: "usage limit"
        return AccountStatus("Out of its $reason · back in ${timeUntil(until, nowMs)}", AccountTone.DANGER)
    }
    if (account.state in setOf("checking", "unknown", "")) return AccountStatus("Checking…", AccountTone.FAINT)
    if (account.inUse) return AccountStatus("In use", AccountTone.WORKING, dot = true)
    return AccountStatus("Ready", AccountTone.MUTED)
}

/** What the account choice calls letting the host choose, as the desktop does. */
internal const val AUTOMATIC_ACCOUNT = "Automatic: the first account with room left"

/**
 * The accounts a chat on [provider] can be set to run on: those that finished
 * signing in, and only while there are two or more of them.
 */
internal fun chatAccountChoices(provider: AgentAuthProviderReply?): List<AgentAccountReply> {
    val accounts = provider?.accounts.orEmpty().filter { !it.pending }
    return if (accounts.size >= 2) accounts else emptyList()
}

/**
 * By assistant, the accounts a new chat can be set to run on, on a host that
 * can set one; null until the host has listed them.
 */
@Composable
internal fun rememberChatAccounts(model: MainViewModel): Map<String, List<AgentAccountReply>>? {
    val auth by model.agentAuth.collectAsStateWithLifecycle()
    val tree by model.client.tree.collectAsStateWithLifecycle()
    // Asked for once the host says it can set a chat's account, which it may
    // only say after the form has opened.
    LaunchedEffect(tree.chatAccounts) { model.loadChatAccounts() }
    if (!tree.chatAccounts) return emptyMap()
    return auth?.mapValues { (_, provider) -> chatAccountChoices(provider) }
}

/** How a choice of account reads: its label, and what keeps it from running if anything does. */
internal fun chatAccountLabel(account: AgentAccountReply, nowMs: Long): String {
    val status = accountStatus(account, nowMs).text
    return if (status in setOf("Ready", "In use", "Checking…")) account.label else "${account.label} · $status"
}

/** The 5-hour and weekly allowance, as the percentage left, for a signed-in account. */
internal fun accountMeters(account: AgentAccountReply): List<Pair<String, Int>> {
    if (account.state != "signed-in") return emptyList()
    fun left(kind: String) = account.windows
        .firstOrNull { it.kind == kind && '·' !in it.label }
        ?.let { (100.0 - it.usedPercent).coerceIn(0.0, 100.0).roundToInt() }
    return listOfNotNull(left("session")?.let { "5h" to it }, left("weekly")?.let { "Week" to it })
}

/** Facts beside an account's name: its plan, and whether it is the CLI's own sign-in. */
internal fun accountFacts(account: AgentAccountReply): String =
    listOfNotNull(account.plan, "CLI sign-in".takeIf { account.default }).joinToString(" · ")

/** The removal being confirmed, as "provider\naccount", so it survives recreation as a string. */
internal fun removalKey(provider: String, account: String) = "$provider\n$account"

/** The time, moved on each minute, so "back in" counts down while the dialog is open. */
@Composable
internal fun rememberMinuteClock(): Long {
    val now by produceState(System.currentTimeMillis()) {
        while (true) {
            delay(60_000)
            value = System.currentTimeMillis()
        }
    }
    return now
}

/**
 * One assistant's accounts, in the order turns use them: each with what it is
 * doing, its allowance, and signing in or out where the account is; adding
 * another; the steps of a running sign-in; and switching when one runs out.
 */
@Composable
internal fun ProviderAccounts(
    provider: AgentAuthProviderReply,
    model: MainViewModel,
    nowMs: Long,
    confirming: String?,
    onConfirm: (String?) -> Unit,
) {
    val accounts = provider.rankedAccounts()
    val several = accounts.size > 1
    val signingIn = provider.signingIn()
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
            BackendIcon(provider.provider, size = 18.dp)
            Spacer(Modifier.width(10.dp))
            Text(
                provider.displayName,
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (provider.holdsSeveral) {
                TextButton(onClick = { model.addAccount(provider.provider) }, enabled = provider.canAdd) {
                    Icon(painterResource(R.drawable.ic_add), contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("Add account")
                }
            }
        }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(MaterialTheme.shapes.medium)
                .background(MaterialTheme.colorScheme.surfaceContainer)
                .border(1.dp, MaterialTheme.colorScheme.outlineVariant, MaterialTheme.shapes.medium),
        ) {
            accounts.forEachIndexed { index, account ->
                if (index > 0) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                val key = removalKey(provider.provider, account.id)
                AccountRow(
                    provider = provider,
                    account = account,
                    index = index,
                    ranked = accounts.size,
                    nowMs = nowMs,
                    confirming = confirming == key,
                    onAskRemove = { onConfirm(key) },
                    onKeep = { onConfirm(null) },
                    model = model,
                )
            }
            signingIn?.let { account ->
                if (accounts.isNotEmpty()) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                SignInSteps(provider, account, model)
            }
        }
        provider.notice?.let {
            Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
        if (several) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .toggleable(
                        value = provider.autoSwitch,
                        role = Role.Switch,
                        onValueChange = { model.setAccountAutoSwitch(provider.provider, it) },
                    )
                    .padding(vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f).padding(end = 12.dp)) {
                    Text("Switch accounts when one runs out", style = MaterialTheme.typography.bodyMedium)
                    Text(
                        if (provider.provider == "codex") {
                            "At a 5-hour, weekly, or monthly limit, the chat continues on the next account with room. " +
                                "Codex starts a new session there, given the chat's messages."
                        } else {
                            "At a 5-hour or weekly limit, the chat continues on the next account with room, in the same session."
                        },
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                Switch(checked = provider.autoSwitch, onCheckedChange = null)
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AccountRow(
    provider: AgentAuthProviderReply,
    account: AgentAccountReply,
    index: Int,
    ranked: Int,
    nowMs: Long,
    confirming: Boolean,
    onAskRemove: () -> Unit,
    onKeep: () -> Unit,
    model: MainViewModel,
) {
    val several = ranked > 1
    val status = accountStatus(account, nowMs)
    val signals = LocalXdSignals.current
    // The warning before removal is not the account's status.
    val tone = if (confirming) AccountTone.MUTED else status.tone
    val toneColor = when (tone) {
        AccountTone.MUTED -> MaterialTheme.colorScheme.onSurfaceVariant
        AccountTone.FAINT -> signals.faint
        AccountTone.DANGER -> MaterialTheme.colorScheme.error
        AccountTone.WORKING -> signals.working
    }
    val signedOut = account.state == "signed-out" || account.state == "failed" || account.problem == "mismatch"
    val inFlight = account.state == "signing-in" || account.state == "signing-out"
    val facts = accountFacts(account)
    val meters = if (confirming) emptyList() else accountMeters(account)
    val actions = confirming || (!inFlight && !provider.busy)
    Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 4.dp, top = 12.dp, bottom = if (actions) 4.dp else 12.dp)) {
        Row(Modifier.fillMaxWidth()) {
            if (several) {
                Text(
                    (index + 1).toString(),
                    modifier = Modifier.width(22.dp).padding(top = 1.dp),
                    color = signals.faint,
                    style = MaterialTheme.typography.labelMedium,
                )
            }
            Column(Modifier.weight(1f).padding(end = 8.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    account.label,
                    color = MaterialTheme.colorScheme.onSurface,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (facts.isNotEmpty()) {
                    Text(facts, color = signals.faint, style = MaterialTheme.typography.labelMedium)
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (status.dot && !confirming) {
                        Box(Modifier.size(6.dp).clip(CircleShape).background(toneColor))
                        Spacer(Modifier.width(6.dp))
                    }
                    Text(
                        if (confirming) "Signs it out and deletes its saved sign-in from this host." else status.text,
                        color = toneColor,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                if (meters.isNotEmpty()) {
                    Column(Modifier.padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        meters.forEach { (label, left) -> AllowanceMeter(label, left) }
                    }
                }
            }
            // Moving sits beside the account, in one column from row to row.
            if (several && !confirming) {
                Column {
                    IconButton(onClick = { model.moveAccount(provider, index, up = true) }, enabled = index > 0) {
                        Icon(painterResource(R.drawable.ic_chevron_up), contentDescription = "Move ${account.label} up", modifier = Modifier.size(18.dp))
                    }
                    IconButton(onClick = { model.moveAccount(provider, index, up = false) }, enabled = index + 1 < ranked) {
                        Icon(painterResource(R.drawable.ic_chevron_down), contentDescription = "Move ${account.label} down", modifier = Modifier.size(18.dp))
                    }
                }
            }
        }
        if (actions) {
            FlowRow(
                Modifier.fillMaxWidth().padding(end = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.End),
            ) {
                if (confirming) {
                    TextButton(onClick = onKeep) { Text("Keep") }
                    Button(
                        onClick = {
                            onKeep()
                            model.removeAccount(provider.provider, account.id)
                        },
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.error,
                            contentColor = MaterialTheme.colorScheme.onError,
                        ),
                    ) { Text("Remove") }
                } else {
                    if (signedOut) {
                        OutlinedButton(onClick = { model.signIn(provider.provider, account.id) }) {
                            Text(if (account.default) "Sign in" else "Sign in again")
                        }
                    }
                    if (account.default) {
                        if (!signedOut) TextButton(onClick = { model.signOut(provider.provider) }) { Text("Sign out") }
                    } else {
                        TextButton(onClick = onAskRemove) { Text("Remove") }
                    }
                }
            }
        }
    }
}

/** What is left of one allowance: neutral, amber when low, rose when spent. */
@Composable
private fun AllowanceMeter(label: String, left: Int) {
    val signals = LocalXdSignals.current
    val tone: Color = when {
        left <= 0 -> MaterialTheme.colorScheme.error
        left <= 15 -> signals.waiting
        else -> MaterialTheme.colorScheme.onSurfaceVariant
    }
    Row(
        Modifier.fillMaxWidth().semantics(mergeDescendants = true) {},
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, modifier = Modifier.width(40.dp), color = signals.faint, style = MaterialTheme.typography.labelSmall)
        LinearProgressIndicator(
            progress = { left / 100f },
            modifier = Modifier.weight(1f),
            color = tone,
            trackColor = MaterialTheme.colorScheme.surfaceContainerHighest,
            strokeCap = StrokeCap.Round,
            gapSize = 0.dp,
            drawStopIndicator = {},
        )
        Text(
            "$left% left",
            modifier = Modifier.width(64.dp),
            color = if (left <= 15) tone else MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.labelSmall,
            textAlign = TextAlign.End,
        )
    }
}

/** The steps of a sign-in in progress, inside the assistant's panel. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SignInSteps(provider: AgentAuthProviderReply, account: AgentAccountReply, model: MainViewModel) {
    // Said in the panel itself: this is a dialog, a window of its own that
    // the app's snackbar would sit behind.
    var notice by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(notice) {
        if (notice != null) {
            delay(NOTICE_MILLIS)
            notice = null
        }
    }
    CompositionLocalProvider(LocalNotice provides { notice = it }) {
        SignInStepsContent(provider, account, model, notice)
    }
}

/** How long a line such as "Copied" stays up: about as long as a short snackbar. */
private const val NOTICE_MILLIS = 4_000L

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun SignInStepsContent(
    provider: AgentAuthProviderReply,
    account: AgentAccountReply,
    model: MainViewModel,
    notice: String?,
) {
    val openLink = rememberLinkOpener()
    val copy = rememberCopier()
    val drafts by model.signInDrafts.collectAsStateWithLifecycle()
    val draft = drafts[provider.provider]?.takeIf { it.attempt == provider.signInAttempt() } ?: SignInDraft()
    val sending = draft.sending
    val waiting = account.loginUrl == null && account.deviceCode == null
    Column(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .padding(horizontal = 14.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                if (account.pending) "Add a ${provider.displayName} account" else "Sign in to ${account.label}",
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.Medium,
            )
            if (account.pending) {
                Text(
                    "Sign in with the account you want to add. If your browser is already signed in to another one, " +
                        "switch accounts there or use a private window.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        if (waiting) {
            Text(
                account.detail ?: "Starting sign-in…",
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        account.deviceCode?.let { code ->
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    code,
                    modifier = Modifier
                        .align(Alignment.CenterVertically)
                        .clip(MaterialTheme.shapes.small)
                        .background(MaterialTheme.colorScheme.surfaceContainer)
                        .border(1.dp, MaterialTheme.colorScheme.outlineVariant, MaterialTheme.shapes.small)
                        .padding(horizontal = 14.dp, vertical = 8.dp),
                    fontFamily = XdMonoFamily,
                    style = MaterialTheme.typography.titleLarge,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                TextButton(onClick = { copy("Sign-in code", code) }, modifier = Modifier.align(Alignment.CenterVertically)) {
                    Icon(painterResource(R.drawable.ic_copy), contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(6.dp))
                    Text("Copy code")
                }
            }
        }
        account.loginUrl?.let { url ->
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                // If no browser can open it, the address is copied for another device.
                Button(onClick = { openLink(url) }) {
                    Icon(painterResource(R.drawable.ic_external), contentDescription = null, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("Open sign-in page")
                }
                TextButton(onClick = { copy("Sign-in link", url) }) { Text("Copy link") }
            }
            Text(
                when {
                    account.deviceCode != null -> "Enter the code on that page."
                    account.needsInput -> "Then paste the code the page shows."
                    else -> "Waiting for you to finish in the browser…"
                },
                color = LocalXdSignals.current.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (account.needsInput) {
            OutlinedTextField(
                value = draft.text,
                onValueChange = { model.editSignInInput(provider, it) },
                modifier = Modifier.fillMaxWidth(),
                placeholder = { Text("Code from the sign-in page") },
                singleLine = true,
                enabled = !sending,
                colors = xdFieldColors(),
            )
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            TextButton(onClick = { model.cancelSignIn(provider.provider) }) { Text("Cancel") }
            if (account.needsInput) {
                OutlinedButton(
                    // Kept until the host takes it, so a failure can be sent again.
                    onClick = { model.sendSignInInput(provider) },
                    enabled = draft.text.isNotBlank() && !sending,
                ) { Text(if (sending) "Sending…" else "Submit code") }
            }
        }
        notice?.let {
            Text(
                it,
                // Read out when it appears, as a snackbar is.
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

/** Copies text, saying so where the system does not (before Android 13). */
@Composable
private fun rememberCopier(): (String, String) -> Unit {
    val context = LocalContext.current
    val notify = LocalNotice.current
    return { label, text ->
        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
        clipboard?.setPrimaryClip(ClipData.newPlainText(label, text))
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) notify("Copied")
    }
}

