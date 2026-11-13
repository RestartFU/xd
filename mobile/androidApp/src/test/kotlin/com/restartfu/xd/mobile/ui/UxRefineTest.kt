package com.restartfu.xd.mobile.ui

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.ConnectionNotice
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import com.restartfu.xd.syntax.SyntaxLanguage
import com.restartfu.xd.syntax.SyntaxToken
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Who said each message, which chat tab is open, errors that are told and
 * can be tried again, code colours that follow the theme, replies read once,
 * Back from a tool, Try now on the connection line, and the Usage page's width.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxRefineTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null
    private var silent: CoroutineScope? = null

    @After fun close() {
        host?.close()
        silent?.cancel()
    }

    // --- Who said it ---

    @Test fun eachMessageSaysWhoItIsFrom() = chat {
        val user = message("The Android client hammers the host")
        assertEquals("You", user.texts().first())
        val reply = message("I'll start with the connection actor")
        assertEquals("Claude Code", reply.texts().first())
        // One stop per message: the words are merged into it after the name.
        assertTrue(reply.texts().toString(), reply.texts().drop(1).any { it.startsWith("I'll start with the connection actor") })
        // A heading in a reply is still its own heading for a screen reader to jump to.
        assertTrue(
            compose.onNode(hasText("Result"), useUnmergedTree = true).fetchSemanticsNode()
                .config.contains(SemanticsProperties.Heading),
        )
    }

    @Test fun aCodexReplySaysCodex() = chat(chatId = "c1b", backend = "codex", await = "Here is a first draft, grouped by client.") {
        assertEquals("Codex", message("Here is a first draft, grouped by client.").texts().first())
        assertEquals("You", message("Draft the release notes for this week.").texts().first())
    }

    @Test fun aLinkInAReplyStaysALink() {
        val scripted = AuditHost(override = { op, request ->
            if (op == "messages" && request.text("chat") == "c1") {
                messages("assistant" to "Opened [the pull request](https://github.com/restartfu/xd/pull/212) for review.")
            } else {
                null
            }
        })
        chat(scripted, await = "Opened the pull request for review.") {
            val reply = message("Opened the pull request for review.")
            assertEquals("Claude Code", reply.texts().first())
            // The link is still there to follow, inside the message.
            val text = compose.onNode(hasText("Opened the pull request for review."), useUnmergedTree = true)
                .fetchSemanticsNode().config[SemanticsProperties.Text].single()
            assertEquals(1, text.getLinkAnnotations(0, text.length).size)
        }
    }

    // --- Tablet tabs ---

    @Test @Config(qualifiers = "w840dp-h1200dp-320dpi")
    fun tabletTabsSayWhichIsOpen() = app {
        open("Reconnect backoff")
        val tabs = compose.onAllNodes(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)).fetchSemanticsNodes()
        fun tab(title: String) = tabs.single { title in it.texts() }
        assertEquals(true, tab("Fix reconnect backoff").config[SemanticsProperties.Selected])
        assertEquals(false, tab("Release notes").config[SemanticsProperties.Selected])
        assertEquals(false, tab("Flaky pairing test").config[SemanticsProperties.Selected])
        // The chat tabs are one group, so their count and place are told.
        fun SemanticsNode.holds(title: String): Boolean =
            title in texts() || children.any { it.holds(title) }
        val groups = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.SelectableGroup), useUnmergedTree = true)
            .fetchSemanticsNodes()
        assertEquals(1, groups.count { it.holds("Fix reconnect backoff") && it.holds("Release notes") })

        compose.onNode(hasContentDescription("All chats")).performClick()
        idle()
        fun item(title: String) = compose.onAllNodes(hasText(title) and hasClickAction()).onLast().fetchSemanticsNode()
        assertTrue(item("Fix reconnect backoff").descriptions().contains("Open"))
        assertTrue(!item("Release notes").descriptions().contains("Open"))
    }

    // --- Errors ---

    @Test fun aPairingErrorIsAnnounced() {
        content { ConnectScreen(MainViewModel(compose.activity.application, noAnswerClient())) }
        fill(code = "ABCD-EFGH")
        compose.onNode(hasText("Pair and connect")).performClick()
        idle()
        val error = compose.onNode(hasText("Paste the full pairing code copied from the host."), useUnmergedTree = true).fetchSemanticsNode()
        assertEquals(LiveRegionMode.Polite, error.config[SemanticsProperties.LiveRegion])
    }

    @Test fun pairingSaysItIsConnectingAtFullSize() {
        content { ConnectScreen(MainViewModel(compose.activity.application, noAnswerClient())) }
        fill(code = "xd2:ABCD:" + "A".repeat(43))
        val before = compose.onNode(hasText("Pair and connect") and hasClickAction()).fetchSemanticsNode().size.height
        assertTrue("${before}px tall", before >= px(48.dp) - 1)
        compose.onNode(hasText("Pair and connect")).performClick()
        idle()
        val button = compose.onNode(hasText(CONNECTING_LABEL) and hasClickAction()).fetchSemanticsNode()
        assertEquals(before, button.size.height)
    }

    @Test fun aProjectsErrorIsAnnouncedAndCanBeTriedAgain() {
        val failing = AtomicBoolean(false)
        val scripted = AuditHost(override = { op, _ ->
            if (op == "tree" && failing.get()) """{"ok":false,"error":"The host could not read its projects."}""" else null
        })
        app(scripted) {
            failing.set(true)
            checkNotNull(host).push("""{"event":"tree"}""")
            await { compose.onAllNodes(hasText("The host could not read its projects."), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty() }
            val error = compose.onNode(hasText("The host could not read its projects."), useUnmergedTree = true).fetchSemanticsNode()
            assertEquals(LiveRegionMode.Polite, error.config[SemanticsProperties.LiveRegion])
            val reads = treeReads()
            failing.set(false)
            compose.onNode(hasText("Try again")).performClick()
            await { treeReads() > reads && compose.onAllNodes(hasText("The host could not read its projects.")).fetchSemanticsNodes().isEmpty() }
            compose.onNode(hasText("Try again")).assertDoesNotExist()
        }
    }

    @Test fun aChangesErrorIsAnnouncedAndCanBeTriedAgain() {
        val failing = AtomicBoolean(true)
        val scripted = AuditHost(override = { op, _ ->
            if (op == "diff-read" && failing.get()) """{"ok":false,"error":"Git is busy."}""" else null
        }).connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val diff = DiffViewModel { session }
        content { DiffPaneContent(diff) }
        await { diff.state.value.error != null }
        idle()
        paneErrorIsTold("Git is busy.")
        failing.set(false)
        compose.onNode(hasText("Try again")).performClick()
        await { diff.state.value.error == null && !diff.state.value.loading }
        idle()
        compose.onNode(hasText("No changes.")).assertExists()
    }

    @Test fun aFolderThatDidNotListIsAnnouncedAndCanBeTriedAgain() {
        val failing = AtomicBoolean(true)
        val scripted = AuditHost(override = { op, _ ->
            if (op == "file-browse" && failing.get()) """{"ok":false,"error":"The folder could not be read."}""" else null
        }).connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val files = FilesViewModel { session }
        content { FilesPaneContent(files) }
        await { files.state.value.error != null }
        idle()
        paneErrorIsTold("The folder could not be read.")
        failing.set(false)
        compose.onNode(hasText("Try again")).performClick()
        await { compose.onAllNodes(hasText("mobile")).fetchSemanticsNodes().isNotEmpty() }
        compose.onNode(hasText("Try again")).assertDoesNotExist()
    }

    @Test fun aTerminalThatDidNotOpenIsAnnouncedAndCanBeTriedAgain() {
        val failing = AtomicBoolean(true)
        val scripted = AuditHost(override = { op, _ ->
            if (op == "terminal-list" && failing.get()) """{"ok":false,"error":"The host could not start a shell."}""" else null
        }).connect().also { host = it }
        val terminal = TerminalViewModel.ShellFactory(scripted.client, "global:mobile").create(TerminalViewModel::class.java)
        content { TerminalPaneContent(terminal, showSessionBar = true) }
        await { terminal.state.value.error != null }
        idle()
        paneErrorIsTold("The host could not start a shell.")
        compose.onNode(hasText("Reopen")).assertDoesNotExist()
        failing.set(false)
        compose.onNode(hasText("Try again")).performClick()
        await { terminal.state.value.id != null }
    }

    // --- Code colours ---

    @Test fun codeColoursFollowAThemeSwitch() {
        var theme by mutableStateOf(MinimalThemePreset.DARK)
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(theme) { Box { CodeText("fun main() = \"hi\"", SyntaxLanguage.KOTLIN) } } }
        fun keyword(): Color? {
            val text = compose.onNode(hasText("fun main() = \"hi\""), useUnmergedTree = true)
                .fetchSemanticsNode().config[SemanticsProperties.Text].single()
            return text.spanStyles.firstOrNull { it.start == 0 && it.end == 3 }?.item?.color
        }
        await { keyword() != null }
        assertEquals(xdSyntax(MinimalThemePreset.DARK).keyword, keyword())
        change { theme = MinimalThemePreset.LIGHT }
        await { keyword() == xdSyntax(MinimalThemePreset.LIGHT).keyword }
        change { theme = MinimalThemePreset.DARK }
        await { keyword() == xdSyntax(MinimalThemePreset.DARK).keyword }
    }

    @Test fun everyLightSyntaxColourReadsOnTheCodeBackground() {
        val light = xdSyntax(MinimalThemePreset.LIGHT)
        val scheme = minimalColors(MinimalThemePreset.LIGHT)
        val signals = xdSignals(MinimalThemePreset.LIGHT)
        // The code block, the diff rows' tints, and the bare page under a file being edited.
        val grounds = listOf(scheme.surfaceVariant, signals.diffAdded, signals.diffRemoved, xdGlass(MinimalThemePreset.LIGHT).backdrop)
        for (token in SyntaxToken.entries.filter { it != SyntaxToken.TEXT }) {
            for (ground in grounds) {
                val ratio = contrastRatio(checkNotNull(light.colour(token)), ground)
                assertTrue("$token is $ratio:1 on $ground", ratio >= 4.5f)
            }
        }
        // Dark is the shared palette, colour for colour.
        val dark = xdSyntax(MinimalThemePreset.DARK)
        for (token in SyntaxToken.entries) {
            val hex = token.colour?.let { Color(it.removePrefix("#").toLong(16) or 0xFF000000L) }
            assertEquals(token.name, hex, dark.colour(token))
        }
    }

    // --- Replies read once ---

    @Test fun aReplyShownAgainIsNotReadAgain() {
        val rows = (1..24).flatMap { i ->
            listOf(
                "user" to "Question $i about the backoff curve.",
                "assistant" to "## Answer $i\n\n" + "The delay doubles after each failure and stops at the cap. ".repeat(6) +
                    "\n\n```kotlin\nfun delay$i(attempt: Int): Long = minOf(2_000L shl attempt, 120_000L)\n```",
            )
        }
        val scripted = AuditHost(override = { op, request ->
            if (op == "messages" && request.text("chat") == "c1") messages(*rows.toTypedArray()) else null
        })
        chat(scripted, await = "Question 24 about the backoff curve.") {
            val list = compose.onNode(hasScrollToIndexAction())
            // Down to the end and back up once, so every row has been read.
            scroll { list.performScrollToIndex(0) }
            scroll { list.performScrollToIndex(rows.size - 1) }
            val parses = ReplyBodies.computed.get()
            val colourings = CodeColours.computed.get()
            // The first replies leave the screen, their rows are let go, and they come back.
            scroll { list.performScrollToIndex(0) }
            await { compose.onAllNodes(hasText("Question 1 about the backoff curve."), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty() }
            scroll { list.performScrollToIndex(rows.size - 1) }
            await { compose.onAllNodes(hasText("Question 24 about the backoff curve."), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty() }
            assertEquals("replies read again", parses, ReplyBodies.computed.get())
            assertEquals("code coloured again", colourings, CodeColours.computed.get())
            // And what is drawn is the read reply, not its plain text.
            compose.onNode(hasText("Answer 24"), useUnmergedTree = true).assertExists()
        }
    }

    // --- Back from a tool ---

    @Test @Config(qualifiers = "w700dp-h1000dp-320dpi")
    fun backFromAToolTabReturnsToTheChat() = app {
        open("Reconnect backoff")
        compose.onNode(hasText("Changes") and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)).performClick()
        idle()
        await { compose.onAllNodes(hasText("No changes.")).fetchSemanticsNodes().isNotEmpty() }
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        idle()
        // Still in the session, on its chat.
        await { compose.onAllNodes(hasText("3 tool calls")).fetchSemanticsNodes().isNotEmpty() }
        compose.onNode(hasText("No changes.")).assertDoesNotExist()
        val chatTab = compose.onNode(hasText("Chat") and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab)).fetchSemanticsNode()
        assertEquals(true, chatTab.config[SemanticsProperties.Selected])
    }

    // --- Try now ---

    @Test fun aWaitToReconnectCanBeCutShort() {
        val tries = AtomicInteger()
        val notice = mutableStateOf<ConnectionNotice?>(ConnectionNotice("Reconnecting in 2 min", "Connection refused", true, waiting = true))
        content {
            CompositionLocalProvider(LocalConnectionNotice provides notice, LocalConnectionRetry provides { tries.incrementAndGet(); Unit }) {
                Column(Modifier.fillMaxWidth().testTag("line")) { ConnectionLine() }
            }
        }
        compose.onNode(hasText(TRY_NOW_LABEL)).performClick()
        idle()
        assertEquals(1, tries.get())
        // The line grows only to the action's full touch target, and its hairline.
        val height = compose.onNode(hasTestTag("line")).fetchSemanticsNode().size.height
        assertTrue("line is ${height}px", height <= px(49.dp) + 1)
        val action = compose.onNode(hasText(TRY_NOW_LABEL) and hasClickAction()).fetchSemanticsNode().touchBoundsInRoot
        assertTrue("Try now is ${action.height}px tall", action.height >= px(48.dp) - 1)
        // An attempt under way has nothing to cut short.
        change { notice.value = ConnectionNotice("Reconnecting…", "Connection refused", true) }
        settle()
        compose.onNode(hasText(TRY_NOW_LABEL)).assertDoesNotExist()
    }

    @Test fun theAppOffersTryNowWhileItWaits() = app(link = Link.Down("Connection refused", 120_000)) {
        compose.onNode(hasText(TRY_NOW_LABEL) and hasClickAction()).assertExists()
    }

    // --- Usage ---

    @Test @Config(qualifiers = "w1280dp-h800dp-320dpi")
    fun theUsagePageKeepsToTheReadingMeasure() {
        val scripted = AuditHost().connect().also { host = it }
        val usage = UsageViewModel(scripted.client, MobileSettings(compose.activity))
        content { UsageContent(usage) }
        idle()
        val root = compose.onRoot().fetchSemanticsNode().boundsInRoot
        val edit = compose.onNode(hasText("Edit")).fetchSemanticsNode().boundsInRoot
        val title = compose.onNode(hasText("Model prices")).fetchSemanticsNode().boundsInRoot
        val width = edit.right - title.left
        assertTrue("content is ${width}px wide", width <= px(ReadingMeasure) + 1)
        // In the middle of the window.
        assertEquals(title.left - root.left, root.right - edit.right, px(48.dp))
    }

    // --- Helpers ---

    private fun paneErrorIsTold(message: String) {
        val error = compose.onNode(hasText(message), useUnmergedTree = true).fetchSemanticsNode()
        assertEquals(LiveRegionMode.Polite, error.config[SemanticsProperties.LiveRegion])
        compose.onNode(hasText("Try again") and hasClickAction()).assertExists()
    }

    private fun treeReads(): Int = checkNotNull(host).sent.count { (it["op"] as? JsonPrimitive)?.content == "tree" }

    private fun fill(code: String) {
        compose.onNode(hasTestTag(PAIRING_ADDRESS_TAG)).performTextReplacement("192.168.1.20")
        compose.onNode(hasTestTag(PAIRING_CODE_TAG)).performTextReplacement(code)
        settle()
    }

    /** A client with no pairing whose host never answers. */
    private fun noAnswerClient(): XdClient {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default).also { silent = it }
        return XdClient({
            object : PlatformSocket {
                override fun connect(connection: SshConnection, listener: PlatformSocketListener) = Unit
                override fun send(bytes: ByteArray) = Unit
                override fun close() = Unit
            }
        }, object : CredentialStore {
            override suspend fun load(): StoredCredentials? = null
            override suspend fun save(credentials: StoredCredentials) = Unit
            override suspend fun clear() = Unit
        }, scope)
    }

    /** The merged node of the message whose words start with [start]. */
    private fun message(start: String): SemanticsNode = compose.onAllNodes(SemanticsMatcher("message starting $start") { node ->
        node.config.getOrNull(SemanticsProperties.Text).orEmpty().any { it.text.startsWith(start) }
    }).fetchSemanticsNodes().single()

    private fun scroll(block: () -> Unit) {
        compose.mainClock.autoAdvance = true
        block()
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        idle()
    }

    private fun content(content: @Composable () -> Unit) {
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(MinimalThemePreset.DARK) { Box { content() } } }
        settle()
    }

    private fun app(scripted: AuditHost = AuditHost(), link: Link? = null, steps: () -> Unit) {
        val activity = compose.activity
        host = scripted.connect()
        val model = MainViewModel(activity.application, scripted.client)
        val settings = MobileSettings(activity)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val live by model.client.link.collectAsStateWithLifecycle()
            XdRoot(MinimalThemePreset.DARK) { MinimalMobileApp(model, link ?: live, settings) }
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        idle()
        steps()
    }

    private fun chat(
        scripted: AuditHost = AuditHost(),
        chatId: String = "c1",
        backend: String = "claude",
        await: String = "3 tool calls",
        steps: () -> Unit,
    ) {
        host = scripted.connect()
        val model = ChatViewModel(scripted.client, chatId)
        val chats = listOf(ChatSummary(chatId, "xd", "Chat", backend, false, sessionId = "s1"))
        content {
            ChatScreen(
                model = model,
                goBack = {},
                sessionTitle = "Reconnect backoff",
                projectName = "xd",
                sessionChats = SessionChats(chats, chatId, {}, {}, {}, {}, {}),
            )
        }
        await { compose.onAllNodes(hasText(await, substring = true), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty() }
        // Until the chat's backend is read, a reply cannot say whose it is.
        await { model.state.value.backend == backend }
        idle()
        steps()
    }

    private fun open(session: String) {
        compose.onAllNodes(hasText(session), useUnmergedTree = true).onLast().performClick()
        await { compose.onAllNodes(hasText("3 tool calls")).fetchSemanticsNodes().isNotEmpty() }
        idle()
    }

    private fun settle() = repeat(10) {
        Thread.sleep(15)
        compose.mainClock.advanceTimeBy(50)
    }

    private fun idle() = repeat(3) {
        ShadowLooper.idleMainLooper()
        settle()
    }

    private fun await(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(condition()) { "Timed out" }
    }

    private fun change(block: () -> Unit) {
        compose.runOnUiThread {
            block()
            androidx.compose.runtime.snapshots.Snapshot.sendApplyNotifications()
        }
    }

    private fun px(size: androidx.compose.ui.unit.Dp): Float = with(compose.density) { size.toPx() }

    private fun SemanticsNode.descriptions(): List<String> =
        config.getOrNull(SemanticsProperties.ContentDescription).orEmpty()

    private fun SemanticsNode.texts(): List<String> =
        config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text }

    private fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content

    private companion object {
        fun messages(vararg rows: Pair<String, String>): String =
            """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                rows.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
    }
}
