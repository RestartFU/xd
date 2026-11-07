package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.OLDER_HOST_PRICES
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.UsageAccountReply
import com.restartfu.xd.protocol.UsageBucketReply
import com.restartfu.xd.protocol.UsagePriceReply
import com.restartfu.xd.protocol.UsageSummaryReply
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Cost and tokens by provider, model, and day, and each account's
 * subscription limits: the desktop's Usage page on the phone.
 */
@Composable
internal fun UsageContent(model: UsageViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    var pricesOpen by rememberSaveable { mutableStateOf(false) }
    var confirmingReset by remember { mutableStateOf<UsageAccountReply?>(null) }
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp)
            .navigationBarsPadding(),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Ops.USAGE_DAYS.forEach { days ->
                FilterChip(
                    selected = state.days == days,
                    onClick = { model.showDays(days) },
                    label = { Text(if (days == 1) "Past 24h" else "$days days") },
                )
            }
        }
        val summary = state.summary
        when {
            summary == null && state.summaryError != null -> ErrorLine(state.summaryError.orEmpty(), model::refresh)
            summary == null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CircularProgressIndicator(Modifier.height(18.dp))
                Text("Reading usage…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            else -> SummarySection(summary, state.summaryLoading, model)
        }
        state.summaryError?.takeIf { summary != null }?.let { ErrorLine(it, model::refresh) }
        HorizontalDivider()
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Limits", modifier = Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = model::readLimits, enabled = !state.limitsLoading) {
                Text(if (state.limitsLoading) "Refreshing…" else "Refresh limits")
            }
        }
        val accounts = state.accounts
        when {
            accounts == null && state.limitsError != null -> ErrorLine(state.limitsError.orEmpty(), model::readLimits)
            accounts == null -> Text("Reading limits…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            accounts.isEmpty() -> Text("No providers", color = MaterialTheme.colorScheme.onSurfaceVariant)
            else -> accounts.forEach { account ->
                val unsettled = account.accountKey in state.unsettledResets
                LimitsCard(
                    account,
                    resetting = state.resetting,
                    unsettled = unsettled,
                    // Checking an earlier attempt again spends nothing new, so it needs no question.
                    onReset = { if (unsettled) account.accountKey?.let(model::useResetCredit) else confirmingReset = account },
                )
            }
        }
        state.limitsError?.takeIf { accounts != null }?.let { ErrorLine(it, model::readLimits) }
        state.resetMessage?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        HorizontalDivider()
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Model prices", style = MaterialTheme.typography.titleMedium)
                Text(
                    "Custom prices for models the price list does not know, per 1M tokens.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            TextButton(onClick = { pricesOpen = true }) { Text("Edit") }
        }
    }
    confirmingReset?.let { account ->
        AlertDialog(
            onDismissRequest = { confirmingReset = null },
            title = { Text("Use a reset credit?") },
            text = {
                Text(
                    "This spends one of ${account.plan ?: "Codex"}'s banked reset credits and clears " +
                        "its usage windows. It cannot be undone.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmingReset = null
                    account.accountKey?.let(model::useResetCredit)
                }) { Text("Use credit") }
            },
            dismissButton = { TextButton(onClick = { confirmingReset = null }) { Text("Cancel") } },
        )
    }
    if (pricesOpen) ModelPricesDialog(model, onDismiss = { pricesOpen = false })
}

@Composable
private fun ErrorLine(message: String, retry: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(message, modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        TextButton(onClick = retry) { Text("Try again") }
    }
}

@Composable
private fun SummarySection(summary: UsageSummaryReply, loading: Boolean, model: UsageViewModel) {
    val signals = LocalXdSignals.current
    val cost = summary.buckets.sumOf(UsageBucketReply::costUsd)
    val tokens = summary.buckets.sumOf { it.totals.all }
    val unpriced = summary.buckets.sumOf(UsageBucketReply::unpricedRecords)
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(summary.windowLabel, modifier = Modifier.weight(1f), color = signals.faint, style = MaterialTheme.typography.labelMedium)
            TextButton(onClick = model::refreshPrices, enabled = !loading) {
                Text(if (loading) "Refreshing…" else "Refresh usage")
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
            Stat("Cost", usd(cost))
            Stat("Processed tokens", compact(tokens))
            Stat("Sessions", summary.sessions.values.sum().toString())
        }
        // Before the empty case too: a partial read is not "no activity".
        summary.sources.filter { it.status == "partial" }.forEach { source ->
            Text(
                "Some ${if (source.provider == "claude") "Claude" else "Codex"} transcripts could not be read, " +
                    "so these totals may be incomplete." + (source.message?.let { " $it" } ?: ""),
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        if (summary.buckets.isEmpty()) {
            Text("No activity in this window.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            return@Column
        }
        CostChart(summary)
        if (unpriced > 0) {
            Text(
                "API estimate excludes $unpriced unpriced records.",
                color = signals.faint,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Text("Models", style = MaterialTheme.typography.titleSmall)
        summary.buckets
            .groupBy { it.provider to it.model }
            .map { (key, buckets) -> Triple(key, buckets.sumOf(UsageBucketReply::costUsd), buckets.sumOf { it.totals.all }) }
            .sortedByDescending { it.second }
            .forEach { (key, modelCost, modelTokens) ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    BackendIcon(key.first, size = 16.dp)
                    Text(key.second, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontFamily = XdMonoFamily, style = MaterialTheme.typography.bodySmall)
                    Text(compact(modelTokens), color = signals.faint, style = MaterialTheme.typography.labelMedium)
                    Text(usd(modelCost), fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.labelLarge)
                }
            }
    }
}

@Composable
private fun Stat(label: String, value: String) {
    Column {
        Text(value, style = MaterialTheme.typography.titleLarge)
        Text(label, color = LocalXdSignals.current.faint, style = MaterialTheme.typography.labelMedium)
    }
}

/** Cost per day, or per hour over the past day, one bar per period. */
@Composable
private fun CostChart(summary: UsageSummaryReply) {
    val byPeriod = summary.buckets.groupBy(UsageBucketReply::period).mapValues { (_, it) -> it.sumOf(UsageBucketReply::costUsd) }
    val values = summary.periods.map { byPeriod[it.key] ?: 0.0 }
    val peak = values.maxOrNull()?.takeIf { it > 0 } ?: 1.0
    val bar = MaterialTheme.colorScheme.primary
    val track = MaterialTheme.colorScheme.surfaceContainerHigh
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Canvas(Modifier.fillMaxWidth().height(96.dp)) {
            if (values.isEmpty()) return@Canvas
            val slot = size.width / values.size
            val width = (slot * 0.7f).coerceAtLeast(1f)
            values.forEachIndexed { index, value ->
                val x = index * slot + (slot - width) / 2
                drawRoundRect(track, Offset(x, 0f), Size(width, size.height), CornerRadius(2f, 2f))
                val height = (value / peak * size.height).toFloat()
                if (height > 0f) {
                    drawRoundRect(bar, Offset(x, size.height - height), Size(width, height), CornerRadius(2f, 2f))
                }
            }
        }
        Row {
            Text(summary.periods.firstOrNull()?.label.orEmpty(), modifier = Modifier.weight(1f), color = LocalXdSignals.current.faint, style = MaterialTheme.typography.labelSmall)
            Text(summary.periods.lastOrNull()?.label.orEmpty(), color = LocalXdSignals.current.faint, style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun LimitsCard(account: UsageAccountReply, resetting: Boolean, unsettled: Boolean, onReset: () -> Unit) {
    val signals = LocalXdSignals.current
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            BackendIcon(account.provider, size = 16.dp)
            Text(
                if (account.provider == "claude") "Claude" else "Codex",
                modifier = Modifier.weight(1f),
                fontWeight = FontWeight.SemiBold,
            )
            account.plan?.let { Text(it, color = signals.faint, style = MaterialTheme.typography.labelMedium) }
        }
        account.unavailable?.let { unavailable ->
            Text(
                unavailable.message ?: "Limits are not available.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        account.windows.forEach { window ->
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row {
                    Text(window.label, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                    Text("${window.usedPercent.coerceIn(0.0, 100.0).toInt()}%", style = MaterialTheme.typography.labelMedium)
                }
                LinearProgressIndicator(
                    progress = { (window.usedPercent / 100.0).coerceIn(0.0, 1.0).toFloat() },
                    modifier = Modifier.fillMaxWidth(),
                    color = if (window.usedPercent >= 90) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
                )
                window.resetsAtLabel?.let { Text("Resets $it", color = signals.faint, style = MaterialTheme.typography.labelSmall) }
            }
        }
        val credits = account.resetCredits?.availableCount ?: 0
        if (account.accountKey != null && (unsettled || credits > 0)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    // An earlier attempt's answer was lost; asking again settles that
                    // same attempt, whatever credits are left now.
                    if (unsettled) "A reset is waiting for the host's answer."
                    else "$credits reset credit${if (credits == 1) "" else "s"}",
                    modifier = Modifier.weight(1f),
                    style = MaterialTheme.typography.bodySmall,
                )
                OutlinedButton(onClick = onReset, enabled = !resetting) {
                    Text(
                        when {
                            resetting -> "Resetting…"
                            unsettled -> "Check again"
                            else -> "Use a credit"
                        },
                    )
                }
            }
        }
    }
}

/**
 * A price row being edited; text so a half-typed number is kept. Its [id]
 * never changes, unlike the editable model name, so removing a row never moves
 * another row's fields, focus included, onto a neighbour.
 */
private data class PriceRow(
    val model: String,
    val input: String,
    val output: String,
    val cacheRead: String,
    val cacheWrite: String,
    val id: String = java.util.UUID.randomUUID().toString(),
)

/** Null (not loaded yet) is saved as an empty list with no marker, so it reads back as null. */
private val PriceRowsSaver = androidx.compose.runtime.saveable.listSaver<List<PriceRow>?, String>(
    save = { rows ->
        rows?.let { listOf(PRICE_ROWS_MARKER) + it.flatMap { row -> listOf(row.id, row.model, row.input, row.output, row.cacheRead, row.cacheWrite) } }
            ?: emptyList()
    },
    restore = { saved ->
        if (saved.firstOrNull() != PRICE_ROWS_MARKER) null
        else saved.drop(1).chunked(6).filter { it.size == 6 }.map { PriceRow(it[1], it[2], it[3], it[4], it[5], id = it[0]) }
    },
)

private const val PRICE_ROWS_MARKER = "loaded-with-ids"


/**
 * The host's custom model prices, in USD per million tokens. Mappings from
 * one model to another are not sent, so the host keeps them as they are now.
 */
@Composable
private fun ModelPricesDialog(model: UsageViewModel, onDismiss: () -> Unit) {
    val usage by model.state.collectAsStateWithLifecycle()
    // Saved, so rotating keeps unsaved edits; prices are not secret.
    var rows by rememberSaveable(stateSaver = PriceRowsSaver) { mutableStateOf<List<PriceRow>?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    // Whether this host keeps its model mappings when a save leaves them out;
    // without that, saving from here could revert another client's mappings.
    var hostKeepsAliases by rememberSaveable { mutableStateOf(false) }
    var attempt by remember { mutableStateOf(0) }
    // The save runs in the view model; the editor closes when its own save
    // lands, matched by an opening token that survives the process too.
    val opening = rememberSaveable { java.util.UUID.randomUUID().toString() }
    val saving = usage.pricesSaving
    val error = loadError ?: usage.pricesError?.takeIf { it.first == opening }?.second
    val close = {
        model.closePrices(opening)
        onDismiss()
    }
    LaunchedEffect(opening in usage.pricesSaved) {
        if (opening in usage.pricesSaved) close()
    }
    LaunchedEffect(attempt) {
        // A draft restored after recreation is kept, not replaced by the host's list.
        if (rows != null) return@LaunchedEffect
        loadError = null
        try {
            val prices = model.prices()
            hostKeepsAliases = prices.aliasesOptional
            rows = prices.prices.map { (id, price) ->
                PriceRow(id, price.input.plain(), price.output.plain(), price.cacheRead?.plain().orEmpty(), price.cacheWrite?.plain().orEmpty())
            }
        } catch (failure: CancellationException) {
            throw failure
        } catch (failure: Throwable) {
            loadError = failure.message ?: "Could not read the model prices"
        }
    }
    val parsed = rows?.map { row ->
        row.model.trim() to runCatching {
            UsagePriceReply(
                input = row.input.trim().toDouble(),
                output = row.output.trim().toDouble(),
                cacheRead = row.cacheRead.trim().takeIf(String::isNotEmpty)?.toDouble(),
                cacheWrite = row.cacheWrite.trim().takeIf(String::isNotEmpty)?.toDouble(),
            ).takeIf { price ->
                // The host takes only finite rates; "1e309" reads as infinity.
                listOf(price.input, price.output, price.cacheRead ?: 0.0, price.cacheWrite ?: 0.0).all { it.isFinite() && it >= 0 }
            }
        }.getOrNull()
    }
    val valid = parsed != null && parsed.all { (id, price) -> id.isNotEmpty() && price != null } &&
        parsed.map { it.first }.toSet().size == parsed.size
    AlertDialog(
        onDismissRequest = { if (!saving) close() },
        title = { Text("Model prices") },
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    "Per 1M tokens. A blank cache rate uses the input rate.",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall,
                )
                val current = rows
                when {
                    current == null && error == null -> CircularProgressIndicator(Modifier.align(Alignment.CenterHorizontally))
                    current == null -> TextButton(onClick = { attempt++ }) { Text("Try again") }
                    else -> {
                        if (current.isEmpty()) {
                            Text("No custom prices. Add a row to set one.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        current.forEach { row -> key(row.id) {
                            val update = { changed: PriceRow -> rows = rows?.map { if (it.id == row.id) changed else it } }
                            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    OutlinedTextField(
                                        value = row.model,
                                        onValueChange = { update(row.copy(model = it)) },
                                        modifier = Modifier.weight(1f),
                                        label = { Text("Model ID") },
                                        singleLine = true,
                                        enabled = !saving,
                                    )
                                    TextButton(onClick = { rows = rows?.filter { it.id != row.id } }, enabled = !saving) { Text("Remove") }
                                }
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    PriceField("Input", row.input, !saving, Modifier.weight(1f)) { update(row.copy(input = it)) }
                                    PriceField("Output", row.output, !saving, Modifier.weight(1f)) { update(row.copy(output = it)) }
                                }
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    PriceField("Cache read", row.cacheRead, !saving, Modifier.weight(1f)) { update(row.copy(cacheRead = it)) }
                                    PriceField("Cache write", row.cacheWrite, !saving, Modifier.weight(1f)) { update(row.copy(cacheWrite = it)) }
                                }
                            }
                        } }
                        TextButton(onClick = { rows = rows.orEmpty() + PriceRow("", "", "", "", "") }, enabled = !saving) { Text("Add model price") }
                    }
                }
                if (rows != null && !hostKeepsAliases) {
                    Text(OLDER_HOST_PRICES, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    model.savePrices(opening, parsed.orEmpty().associate { (id, price) -> id to checkNotNull(price) })
                },
                enabled = valid && !saving && hostKeepsAliases,
            ) { Text(if (saving) "Saving…" else "Save changes") }
        },
        dismissButton = { TextButton(onClick = close, enabled = !saving) { Text("Cancel") } },
    )
}

@Composable
private fun PriceField(label: String, value: String, enabled: Boolean, modifier: Modifier, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        modifier = modifier,
        label = { Text(label) },
        singleLine = true,
        enabled = enabled,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
    )
}

// A whole rate drops its ".0", but only where a Long holds it exactly; past
// that, toLong() would saturate and a save would change the rate.
private fun Double.plain(): String =
    if (this == Math.floor(this) && kotlin.math.abs(this) < 1e15) toLong().toString() else toString()

private fun usd(value: Double): String = when {
    value >= 100 -> String.format(Locale.US, "$%,.0f", value)
    value >= 1 -> String.format(Locale.US, "$%,.2f", value)
    value > 0 -> String.format(Locale.US, "$%.3f", value)
    else -> "$0"
}

private fun compact(value: Long): String = when {
    value >= 1_000_000_000 -> String.format(Locale.US, "%.1fB", value / 1e9)
    value >= 1_000_000 -> String.format(Locale.US, "%.1fM", value / 1e6)
    value >= 1_000 -> String.format(Locale.US, "%.1fK", value / 1e3)
    else -> value.toString()
}
