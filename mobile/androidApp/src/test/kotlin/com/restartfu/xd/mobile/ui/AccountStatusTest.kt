package com.restartfu.xd.mobile.ui

import com.restartfu.xd.protocol.AgentAccountReply
import com.restartfu.xd.protocol.AgentAccountWindowReply
import com.restartfu.xd.protocol.AgentAuthProviderReply
import com.restartfu.xd.protocol.UsageAccountReply
import org.junit.Assert.assertEquals
import org.junit.Test

/** The account rows' wording, which follows the desktop's. */
class AccountStatusTest {
    private val now = 1_000_000_000L

    @Test fun rowsSayWhatEachAccountIsDoing() {
        val inUse = AgentAccountReply("default", state = "signed-in", inUse = true)
        assertEquals(AccountStatus("In use", AccountTone.WORKING, dot = true), accountStatus(inUse, now))
        val spent = AgentAccountReply(
            "a", state = "signed-in", limitedUntilMs = now + 2 * 3_600_000 + 5 * 60_000, limitedReason = "5-hour limit",
        )
        assertEquals(
            AccountStatus("Out of its 5-hour limit · back in 2 h 5 min", AccountTone.DANGER),
            accountStatus(spent, now),
        )
        // A limit that has passed no longer holds the account back.
        assertEquals("Ready", accountStatus(spent.copy(limitedUntilMs = now - 1), now).text)
        val moved = AgentAccountReply("a", state = "signed-in", problem = "mismatch")
        assertEquals(
            AccountStatus("Now signed in to a different account, so it is not used", AccountTone.DANGER),
            accountStatus(moved, now),
        )
        assertEquals(
            "Same login as an account above, so it is not used",
            accountStatus(moved.copy(problem = "duplicate"), now).text,
        )
        assertEquals("Signed out", accountStatus(AgentAccountReply("a", state = "signed-out"), now).text)
        assertEquals(AccountTone.DANGER, accountStatus(AgentAccountReply("a", state = "failed"), now).tone)
        assertEquals("Sign-in failed", accountStatus(AgentAccountReply("a", state = "failed"), now).text)
        assertEquals("Checking…", accountStatus(AgentAccountReply("a", state = "checking"), now).text)
        assertEquals("Signing in…", accountStatus(AgentAccountReply("a", state = "signing-in"), now).text)
    }

    @Test fun timeLeftReadsInWholeUnits() {
        assertEquals("1 min", timeUntil(now + 1_000, now))
        assertEquals("12 min", timeUntil(now + 12 * 60_000, now))
        assertEquals("3 h", timeUntil(now + 3 * 3_600_000, now))
        assertEquals("3 d", timeUntil(now + 3 * 86_400_000L, now))
        assertEquals("1 d 2 h", timeUntil(now + 26 * 3_600_000L, now))
    }

    @Test fun metersShowWhatIsLeftOfASignedInAccount() {
        val account = AgentAccountReply(
            "a",
            state = "signed-in",
            windows = listOf(
                AgentAccountWindowReply("session", "5h", 38.4),
                AgentAccountWindowReply("weekly", "Weekly", 120.0),
                // A pooled sub-window is not the account's own.
                AgentAccountWindowReply("session", "5h · Spark", 90.0),
            ),
        )
        assertEquals(listOf("5h" to 62, "Week" to 0), accountMeters(account))
        assertEquals(emptyList<Pair<String, Int>>(), accountMeters(account.copy(state = "signed-out")))
    }

    @Test fun factsNameThePlanAndTheCliSignIn() {
        assertEquals("Pro · CLI sign-in", accountFacts(AgentAccountReply("default", default = true, plan = "Pro")))
        assertEquals("", accountFacts(AgentAccountReply("a")))
    }

    @Test fun codexComesBeforeClaude() {
        val order = accountProviderOrder(
            listOf(AgentAuthProviderReply("claude", "Claude Code"), AgentAuthProviderReply("codex", "Codex")),
        )
        assertEquals(listOf("codex", "claude"), order.map { it.provider })
    }

    @Test fun limitsSayWhichOfSeveralAccountsTheyAre() {
        assertEquals("Codex · account 2", limitsTitle(UsageAccountReply("codex", accountRank = 2)))
        assertEquals("Claude", limitsTitle(UsageAccountReply("claude")))
    }
}
