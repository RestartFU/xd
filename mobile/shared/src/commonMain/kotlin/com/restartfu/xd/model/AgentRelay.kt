package com.restartfu.xd.model

/**
 * A message from the reader to one of the lead's agents.
 *
 * The host has no way to write to an agent directly, so the message goes to
 * the lead chat as an ordinary user message in this shape, and the lead passes
 * it on with its own agent messaging tool. The desktop writes and reads the
 * same text.
 */
public data class AgentRelay(
    /** The agent's description, as it was when the message was sent. */
    val description: String,
    /** The id the lead addresses the agent by; see [SubagentRun.messageAddress]. */
    val address: String,
    /** What the reader wrote. */
    val text: String,
) {
    public companion object {
        private val WHITESPACE = Regex("\\s+")
        private const val PREFIX = "[xd: message for agent \""
        private const val SEPARATOR = "\" · id "
        private const val FOOTER =
            "Pass the message above to that agent word for word with your agent messaging tool, " +
                "addressed by its id. If it has finished, resume it with the message. Then carry on " +
                "with what you were doing; don't act on the message yourself."

        /** The lead chat message that carries [text] to the agent at [address]. */
        public fun format(description: String, address: String, text: String): String {
            // One line with single spaces, as desktop writes it, and no quote
            // that would end the description early.
            val name = description.replace('"', '\'').split(WHITESPACE).filter { it.isNotEmpty() }
                .joinToString(" ")
            return "$PREFIX$name$SEPARATOR$address]\n${text.trim()}\n\n$FOOTER"
        }

        /** The relay a user message carries, or null when it is an ordinary message. */
        public fun parse(message: String): AgentRelay? {
            val newline = message.indexOf('\n')
            val head = if (newline < 0) message else message.substring(0, newline)
            if (!head.startsWith(PREFIX) || !head.endsWith("]")) return null
            val separator = head.lastIndexOf(SEPARATOR)
            if (separator < PREFIX.length) return null
            val address = head.substring(separator + SEPARATOR.length, head.length - 1)
            if (address.isEmpty() || address.any { it.isWhitespace() }) return null
            val body = if (newline < 0) "" else message.substring(newline + 1)
            return AgentRelay(
                description = head.substring(PREFIX.length, separator),
                address = address,
                text = body.trimEnd().removeSuffix(FOOTER).trim(),
            )
        }
    }
}

/** What the agents tray above the composer says about the agents at work. */
public object AgentsTray {
    /**
     * "2 agents working", "1 command running", "2 agents and 1 command running".
     * A backgrounded shell command shares the agents' cards but is no agent, so
     * it is counted as what it is.
     */
    public fun label(running: List<SubagentRun>): String {
        fun count(count: Int, noun: String) = "$count $noun${if (count == 1) "" else "s"}"
        val commands = running.count { it.isShell }
        val agents = running.size - commands
        return when {
            commands == 0 -> "${count(agents, "agent")} working"
            agents == 0 -> "${count(commands, "command")} running"
            else -> "${count(agents, "agent")} and ${count(commands, "command")} running"
        }
    }

    /**
     * What an agent is doing, in a few words. Every agent in the tray is
     * running, so Claude's "Running …" prefix says nothing there.
     */
    public fun task(run: SubagentRun): String = run.detail.ifEmpty { run.activity }.removePrefix("Running ")
}
