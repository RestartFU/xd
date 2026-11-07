package com.restartfu.xd.mobile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.XdClient
import com.restartfu.xd.net.Link
import com.restartfu.xd.protocol.UsageAccountReply
import com.restartfu.xd.protocol.UsagePriceReply
import com.restartfu.xd.protocol.UsagePricesReply
import com.restartfu.xd.protocol.UsageSummaryReply
import java.time.ZoneId
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class UsageState(
    val days: Int = 7,
    val summary: UsageSummaryReply? = null,
    val summaryLoading: Boolean = false,
    val summaryError: String? = null,
    val accounts: List<UsageAccountReply>? = null,
    val limitsLoading: Boolean = false,
    val limitsError: String? = null,
    val resetting: Boolean = false,
    val resetMessage: String? = null,
    /** Accounts with a reset attempt the host has not settled yet. */
    val unsettledResets: Set<String> = emptySet(),
)

/**
 * The host's Usage page: cost and tokens over a window, and each account's
 * subscription limits, as the desktop's Usage page reads them.
 */
class UsageViewModel(
    private val client: XdClient,
    private val settings: MobileSettings,
) : ViewModel() {
    private val _state = MutableStateFlow(UsageState())
    val state: StateFlow<UsageState> = _state.asStateFlow()
    private var summaryReads = 0L
    private var limitReads = 0L


    init {
        refresh()
        // Usage answers go only to the connection that asked, so one lost to a
        // reconnect is asked for again.
        viewModelScope.launch {
            var wasUp = client.link.value is Link.Up
            client.link.collect { link ->
                val up = link is Link.Up
                if (up && !wasUp) refresh()
                wasUp = up
            }
        }
    }

    private val timeZone: String get() = ZoneId.systemDefault().id

    fun showDays(days: Int) {
        if (_state.value.days == days) return
        _state.update { it.copy(days = days, summary = null) }
        readSummary(refresh = false)
    }

    fun refresh() {
        readSummary(refresh = false)
        readLimits()
    }

    /** Reads again and fetches the price list afresh, as the desktop's Refresh usage does. */
    fun refreshPrices() = readSummary(refresh = true)

    private fun readSummary(refresh: Boolean) {
        val read = ++summaryReads
        val days = _state.value.days
        _state.update { it.copy(summaryLoading = true, summaryError = null) }
        viewModelScope.launch {
            try {
                val summary = client.usageSummary(days, timeZone, refresh)
                if (read == summaryReads) _state.update { it.copy(summary = summary, summaryLoading = false) }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (read == summaryReads) {
                    _state.update {
                        it.copy(summaryLoading = false, summaryError = error.message ?: "The host could not read usage.")
                    }
                }
            }
        }
    }

    fun readLimits() {
        val read = ++limitReads
        _state.update { it.copy(limitsLoading = true, limitsError = null) }
        viewModelScope.launch {
            try {
                val accounts = client.usageLimits(timeZone)
                if (read == limitReads) {
                    _state.update {
                        it.copy(
                            accounts = accounts,
                            limitsLoading = false,
                            unsettledResets = unsettled(accounts),
                        )
                    }
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (read == limitReads) {
                    _state.update {
                        it.copy(limitsLoading = false, limitsError = error.message ?: "Could not read limits.")
                    }
                }
            }
        }
    }

    /** Spends one banked Codex reset credit. The caller has asked the user first. */
    fun useResetCredit(accountKey: String) {
        if (_state.value.resetting) return
        // Each account's unsettled attempt is kept on disk until the host
        // settles it, so retrying after a lost answer, another account, or the
        // app being killed cannot spend a second credit, as the desktop keeps them.
        val attempt = settings.resetAttempt(accountKey) ?: UUID.randomUUID().toString()
        // Committed before every send, not just the first: a failed commit can
        // still leave the key readable in memory without it being on disk.
        if (!settings.saveResetAttempt(accountKey, attempt)) {
            _state.update { it.copy(resetMessage = "Could not keep a record of the reset, so it was not sent.") }
            return
        }
        _state.update { it.copy(resetting = true, resetMessage = null, unsettledResets = it.unsettledResets + accountKey) }
        viewModelScope.launch {
            try {
                val outcome = client.consumeResetCredit(accountKey, attempt)
                settings.clearResetAttempt(accountKey, attempt)
                _state.update {
                    it.copy(
                        resetting = false,
                        resetMessage = resetOutcome(outcome),
                        unsettledResets = it.unsettledResets - accountKey,
                    )
                }
                readLimits()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                // The attempt key stays, so trying again settles this attempt.
                _state.update { it.copy(resetting = false, resetMessage = error.message ?: "Could not use the reset credit.") }
            }
        }
    }

    private fun unsettled(accounts: List<UsageAccountReply>): Set<String> =
        accounts.mapNotNull { it.accountKey }.filter { settings.resetAttempt(it) != null }.toSet()

    suspend fun prices(): UsagePricesReply = client.usagePrices()

    suspend fun savePrices(prices: Map<String, UsagePriceReply>, aliases: Map<String, String>): UsagePricesReply {
        val saved = client.setUsagePrices(prices, aliases)
        readSummary(refresh = false)
        return saved
    }

    class Factory(
        private val client: XdClient,
        private val settings: MobileSettings,
    ) : androidx.lifecycle.ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T = UsageViewModel(client, settings) as T
    }
}

/** The desktop's words for each reset outcome. */
internal fun resetOutcome(outcome: String): String = when (outcome) {
    "reset" -> "Reset applied. Your windows have cleared."
    "nothingToReset" -> "Nothing to reset right now."
    "noCredit" -> "No reset credit left."
    "alreadyRedeemed" -> "That credit was already redeemed."
    else -> "Could not use the reset credit."
}
