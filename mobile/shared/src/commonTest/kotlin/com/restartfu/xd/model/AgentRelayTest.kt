package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AgentRelayTest {
    @Test
    fun formatsTheMessageTheLeadPassesOn() {
        assertEquals(
            "[xd: message for agent \"Review the diff\" · id a22197aff550fdfd9]\n" +
                "Check the tests too.\n\n" +
                "Pass the message above to that agent word for word with your agent messaging tool, " +
                "addressed by its id. If it has finished, resume it with the message. Then carry on " +
                "with what you were doing; don't act on the message yourself.",
            AgentRelay.format("Review the diff", "a22197aff550fdfd9", "  Check the tests too.\n"),
        )
    }

    @Test
    fun keepsTheDescriptionOnOneLineWithoutQuotes() {
        val message = AgentRelay.format("Find \"retry\"\nin net", "id-1", "hi")
        assertTrue(message.startsWith("[xd: message for agent \"Find 'retry' in net\" · id id-1]\n"))
    }

    @Test
    fun writesTheDescriptionAsDesktopDoes() {
        val message = AgentRelay.format("  Read \t the\r\n  parser ", "a1", "hi")
        assertTrue(message.startsWith("[xd: message for agent \"Read the parser\" · id a1]\n"))
    }

    @Test
    fun readsPastWhitespaceAfterTheFooter() {
        val relay = AgentRelay.parse(AgentRelay.format("Explore", "a1", "hi") + "\n  \n")
        assertEquals("hi", relay?.text)
    }

    @Test
    fun needsAnAddressWithoutSpaces() {
        assertEquals(null, AgentRelay.parse(AgentRelay.format("x", "", "hi")))
        assertEquals(null, AgentRelay.parse(AgentRelay.format("x", "a b", "hi")))
    }

    @Test
    fun roundTrips() {
        val relay = AgentRelay.parse(AgentRelay.format("Explore · parser", "thread-9", "Line one\n\nLine two"))
        assertEquals(AgentRelay("Explore · parser", "thread-9", "Line one\n\nLine two"), relay)
    }

    @Test
    fun readsTheAddressAfterTheLastSeparator() {
        val relay = AgentRelay.parse(AgentRelay.format("a\" · id b", "c", "hello").replace("a' · id b", "a\" · id b"))!!
        assertEquals("a\" · id b", relay.description)
        assertEquals("c", relay.address)
        assertEquals("hello", relay.text)
    }

    @Test
    fun needsTheWholeForm() {
        // Quoting the header, or the form without its footer or words, is an
        // ordinary message.
        assertNull(AgentRelay.parse("[xd: message for agent \"x\" · id 1]"))
        assertNull(AgentRelay.parse("[xd: message for agent \"x\" · id 1]\n  just this  \n"))
        assertNull(AgentRelay.parse(AgentRelay.format("x", "1", "  ")))
    }

    @Test
    fun leavesOrdinaryMessagesAlone() {
        assertNull(AgentRelay.parse("hello"))
        assertNull(AgentRelay.parse("[xd: message for agent \"x\" · id 1] trailing\nbody"))
        assertNull(AgentRelay.parse("[xd: message for agent \"x\" id 1]\nbody"))
        assertNull(AgentRelay.parse("say [xd: message for agent \"x\" · id 1]\nbody"))
        assertNull(AgentRelay.parse("[xd: message for agent \" · id 1]\nbody"))
    }

    @Test
    fun countsShellCommandsSeparately() {
        val agent = run("Explore")
        val shell = run("Shell")
        assertEquals("1 agent working", AgentsTray.label(listOf(agent)))
        assertEquals("2 agents working", AgentsTray.label(listOf(agent, agent)))
        assertEquals("1 command running", AgentsTray.label(listOf(shell)))
        assertEquals("2 commands running", AgentsTray.label(listOf(shell, shell)))
        assertEquals("2 agents and 1 command running", AgentsTray.label(listOf(agent, shell, agent)))
    }

    @Test
    fun namesTheTaskWithoutTheRunningPrefix() {
        assertEquals("Review the diff", AgentsTray.task(run("Explore", detail = "Review the diff")))
        assertEquals("cargo test", AgentsTray.task(run("Shell", activity = "Running cargo test")))
        assertEquals("Running", AgentsTray.task(run("Shell", activity = "Running")))
    }

    @Test
    fun addressesClaudeByTaskIdAndCodexByThread() {
        val claude = SubagentRun.parse(
            "subagent_run\n" + """{"key":"toolu_1","provider":"Claude","agent":"Explore","task_id":"a221"}""",
        )!!
        assertEquals("a221", claude.taskId)
        assertEquals("a221", claude.messageAddress)
        assertTrue(claude.canTakeMessages)

        val waiting = SubagentRun.parse("subagent_run\n" + """{"key":"toolu_1","provider":"Claude","agent":"Explore"}""")!!
        assertEquals("", waiting.taskId)
        assertNull(waiting.messageAddress)
        assertFalse(waiting.canTakeMessages)
        assertFalse(waiting.runsAsCommand)

        val codex = SubagentRun.parse("subagent_run\n" + """{"key":"thread-1","provider":"codex","agent":"worker"}""")!!
        assertEquals("thread-1", codex.messageAddress)
        assertTrue(codex.canTakeMessages)
    }

    @Test
    fun commandsTakeNoMessages() {
        val shell = SubagentRun.parse("subagent_run\n" + """{"key":"thread-1","provider":"Codex","agent":"Shell"}""")!!
        assertTrue(shell.isShell)
        assertEquals("thread-1", shell.messageAddress)
        assertFalse(shell.canTakeMessages)

        val launched = SubagentRun.parse(
            "subagent_run\n" + """{"key":"k","provider":"Codex","agent":"worker","launcher":"Claude","task_id":"t"}""",
        )!!
        assertTrue(launched.runsAsCommand)
        assertFalse(launched.canTakeMessages)
    }

    private fun run(agent: String, detail: String = "", activity: String = "") = SubagentRun(
        key = "k", identity = agent, status = "Running", state = SubagentState.RUNNING,
        detail = detail, marker = "", activity = activity,
    )
}
