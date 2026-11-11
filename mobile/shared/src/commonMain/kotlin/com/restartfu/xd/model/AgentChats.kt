package com.restartfu.xd.model

/** How the turn an agent chat reported on ended. */
public enum class AgentOutcome { DONE, FAILED, WAITING }

/**
 * The message xd sends a chat when an agent it started with xd's start_agent
 * tool ends a turn: `[xd] Agent "Title" (chat-id) is done with its turn.`,
 * then the reply. See docs/protocol.md.
 */
public data class AgentReport(
    val chatId: String,
    /** Empty when the agent had no title yet. */
    val title: String,
    val outcome: AgentOutcome,
) {
    public companion object {
        /** The report [text] is, when xd sent it for one of the chat's agents. */
        public fun parse(text: String): AgentReport? {
            val first = text.lineSequence().firstOrNull()?.removePrefix("[xd] ")
                ?.takeIf { text.startsWith("[xd] ") } ?: return null
            val split = first.lastIndexOf(") is ").takeIf { it >= 0 } ?: return null
            val outcome = when (first.substring(split + ") is ".length)) {
                "done with its turn." -> AgentOutcome.DONE
                "stopped with an error." -> AgentOutcome.FAILED
                "waiting for an answer." -> AgentOutcome.WAITING
                else -> return null
            }
            val head = first.substring(0, split)
            val open = head.lastIndexOf(" (").takeIf { it >= 0 } ?: return null
            val chatId = head.substring(open + 2).takeIf { it.isNotEmpty() && ' ' !in it } ?: return null
            val name = head.substring(0, open)
            val title = when {
                name == "An agent" -> ""
                name.startsWith("Agent \"") && name.endsWith("\"") && name.length >= 8 ->
                    name.substring(7, name.length - 1)
                else -> return null
            }
            return AgentReport(chatId, title, outcome)
        }
    }
}

/** How a CI watch xd ran for the chat's agent ended. */
public enum class CiOutcome { PASSED, FAILED, STOPPED }

/**
 * The message xd sends a chat when a CI watch its agent asked for with xd's
 * watch_ci tool ends: `[xd] CI passed on <url> at <head>.`, `[xd] CI failed
 * on …`, or `[xd] Stopped watching CI on <url>.`, then the details. See
 * docs/protocol.md.
 */
public data class CiReport(
    val url: String,
    val number: String,
    val outcome: CiOutcome,
) {
    public companion object {
        /** The CI report [text] is, when xd sent it. */
        public fun parse(text: String): CiReport? {
            val first = text.lineSequence().firstOrNull()?.removePrefix("[xd] ")
                ?.takeIf { text.startsWith("[xd] ") } ?: return null
            val (outcome, rest) = when {
                first.startsWith("CI passed on ") -> CiOutcome.PASSED to first.removePrefix("CI passed on ")
                first.startsWith("CI failed on ") -> CiOutcome.FAILED to first.removePrefix("CI failed on ")
                first.startsWith("Stopped watching CI on ") ->
                    CiOutcome.STOPPED to first.removePrefix("Stopped watching CI on ")
                else -> return null
            }
            val url = when (outcome) {
                CiOutcome.STOPPED -> rest.takeIf { it.endsWith(".") }?.dropLast(1)
                else -> rest.indexOf(" at ").takeIf { it >= 0 }?.let { rest.substring(0, it) }
            } ?: return null
            if (!url.startsWith("https://") || ' ' in url) return null
            val split = url.lastIndexOf("/pull/").takeIf { it >= 0 } ?: return null
            val number = url.substring(split + "/pull/".length)
            if (number.isEmpty() || !number.all { it in '0'..'9' }) return null
            return CiReport(url, number, outcome)
        }
    }
}

/**
 * This chat as its parent shows it when its turn [asking] waits on an
 * approval or answer from the user: waiting on the user rather than working,
 * as it would be after a reply that asked a question.
 */
public fun ChatSummary.waitingOnUser(asking: Boolean): ChatSummary =
    if (asking) copy(working = false, questionWaiting = true) else this

/** How an agent chat stands, in the words its row uses. */
public enum class AgentChatStatus(public val label: String) {
    RUNNING("Running"),
    PAUSED("Paused"),
    NEEDS_YOU("Needs you"),
    DONE("Done"),
    FAILED("Failed"),
    STOPPED("Stopped"),
}

/**
 * How the agent chat behind [run] stands. What the tree says of the chat now,
 * [chat], wins over the card's last turn: a card that says it runs while its
 * chat does not, or whose chat is gone, was stopped or cut short.
 */
public fun agentChatStatus(run: SubagentRun, chat: ChatSummary?): AgentChatStatus = when {
    chat?.working == true -> AgentChatStatus.RUNNING
    chat?.paused == true -> AgentChatStatus.PAUSED
    chat?.questionWaiting == true -> AgentChatStatus.NEEDS_YOU
    run.state == SubagentState.FAILURE -> AgentChatStatus.FAILED
    run.state == SubagentState.SUCCESS -> AgentChatStatus.DONE
    else -> AgentChatStatus.STOPPED
}
