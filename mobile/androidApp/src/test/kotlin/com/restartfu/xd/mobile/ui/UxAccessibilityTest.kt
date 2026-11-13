package com.restartfu.xd.mobile.ui

import android.util.Base64
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.ConnectionNotice
import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * What a screen reader is told, and what a finger can hit: status marks and
 * the rows that carry them, live regions, disclosure rows, the Files pane,
 * the composer's touch targets, the terminal, and the connection line.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxAccessibilityTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null

    @After fun closeHost() {
        host?.close()
    }

    // --- Status marks ---

    @Test fun everyStatusMarkSaysItsName() {
        assertEquals(listOf("Working", "Waiting on you", "Done", "Paused"), StatusMark.entries.map { it.label })
        content { Row { StatusMark.entries.forEach { StatusMarkIcon(it) } } }
        for (mark in StatusMark.entries) compose.onNode(hasContentDescription(mark.label)).assertExists()
    }

    @Test fun aMarkBesideItsWordsSaysNothingItself() {
        content { StatusPill("Waiting on you", androidx.compose.ui.graphics.Color.Unspecified, mark = StatusMark.WAITING) }
        compose.onNode(hasText("Waiting on you")).assertExists()
        compose.onNode(hasContentDescription("Waiting on you")).assertDoesNotExist()
    }

    @Test fun aChatIsMarkedInTheOrderASessionRowUses() {
        val quiet = ChatSummary("c", "f", "Chat", "claude", working = false)
        assertNull(chatStatus(quiet, waiting = false, unread = false))
        assertEquals(StatusMark.DONE, chatStatus(quiet, waiting = false, unread = true))
        assertEquals(StatusMark.PAUSED, chatStatus(quiet.copy(paused = true), waiting = false, unread = true))
        assertEquals(StatusMark.WORKING, chatStatus(quiet.copy(working = true, paused = true), waiting = false, unread = true))
        // Waiting on the reader comes before everything, a quota stop included.
        assertEquals(StatusMark.WAITING, chatStatus(quiet.copy(working = true), waiting = true, unread = false))
        assertEquals(StatusMark.WAITING, chatStatus(quiet.copy(quotaReached = true), waiting = false, unread = false))
        // Waiting on its background tasks has a ring of its own on a session row, and no shared mark.
        assertNull(chatStatus(quiet.copy(backgroundTasks = 2), waiting = false, unread = true))
    }

    @Test fun aClosedRowSaysHowManySessionsStandEachWay() {
        val signals = listOf(
            SessionSignal.WORKING, SessionSignal.WAITING, SessionSignal.WORKING,
            SessionSignal.DONE, SessionSignal.BACKGROUND, SessionSignal.IDLE,
        )
        assertEquals("1 waiting on you, 2 working, 1 waiting on background tasks, 1 done", signalSummary(signals))
        assertEquals("", signalSummary(listOf(SessionSignal.IDLE, SessionSignal.PAUSED)))
    }

    @Test fun sessionRowsSayTheirStatus() = app {
        fun row(title: String) = compose.onNode(hasText(title) and hasClickAction()).fetchSemanticsNode()
        // One of its chats holds an approval: said in words on the row.
        assertTrue(row("Reconnect backoff").texts().contains("Waiting on you"))
        assertTrue(row("Diff pane").texts().contains("Paused"))
        // Marks with no words beside them carry their name.
        assertEquals(listOf("Working"), row("Usage page").status())
        assertEquals(listOf("Done"), row("Pairing flow").status())
        assertEquals(listOf("Waiting on 2 background tasks"), row("Theme tokens").status())
        assertEquals(emptyList<String>(), row("Host update").status())
        // A closed project gathers its sessions' marks, and says them as counts.
        val web = row("web")
        assertEquals("Collapsed", web.config[SemanticsProperties.StateDescription])
        assertTrue(web.descriptions().toString(), "1 waiting on you, 1 working, 1 done" in web.descriptions())
        assertTrue("3 sessions" in web.descriptions())
        assertEquals("Expanded", row("xd").config[SemanticsProperties.StateDescription])
        // Connected: the header's dot says so, and there is no line under it.
        compose.onNode(hasContentDescription("Connected")).assertExists()
        compose.onNode(hasTestTag(CONNECTION_LINE_TAG), useUnmergedTree = true).assertDoesNotExist()
    }

    @Test fun thePhoneChatMenuMarksEachChat() = app {
        open("Reconnect backoff")
        compose.onAllNodes(hasText("Fix reconnect backoff")).onLast().performClick()
        idle()
        fun item(title: String) = compose.onAllNodes(hasText(title) and hasClickAction()).onLast().fetchSemanticsNode()
        assertEquals(listOf("Working"), item("Release notes").status())
        assertEquals(listOf("Waiting on you"), item("Flaky pairing test").status())
        // The open chat is quiet: only its check.
        assertEquals(listOf("Open"), item("Fix reconnect backoff").status())
    }

    @Test @Config(qualifiers = "w840dp-h1200dp-320dpi")
    fun tabletChatTabsMarkEachChat() = app {
        open("Reconnect backoff")
        fun tab(title: String) = compose.onNode(
            hasText(title) and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab),
        ).fetchSemanticsNode()
        assertEquals(listOf("Working"), tab("Release notes").status())
        assertEquals(listOf("Waiting on you"), tab("Flaky pairing test").status())
        assertEquals(emptyList<String>(), tab("Fix reconnect backoff").status())
    }

    // --- The connection line ---

    @Test fun theLineSaysNothingWhileConnected() {
        val notice = mutableStateOf<ConnectionNotice?>(null)
        content { CompositionLocalProvider(LocalConnectionNotice provides notice) { ChatHeader() } }
        compose.onNode(hasTestTag(CONNECTION_LINE_TAG), useUnmergedTree = true).assertDoesNotExist()
        // One 56dp row and its hairline, and nothing more.
        val connected = compose.onNode(hasTestTag("header")).fetchSemanticsNode().size.height
        assertEquals(px(57.dp), connected.toFloat(), 1f)

        change { notice.value = ConnectionNotice("Connecting…", null, failure = false) }
        settle()
        val line = compose.onNode(hasTestTag(CONNECTION_LINE_TAG), useUnmergedTree = true).fetchSemanticsNode()
        assertEquals(LiveRegionMode.Polite, line.config[SemanticsProperties.LiveRegion])
        assertEquals(listOf("Connecting to the host"), line.descriptions())
        val connecting = compose.onNode(hasTestTag("header")).fetchSemanticsNode().size.height
        // Slim: under a 48dp row.
        assertTrue("line is ${connecting - connected}px", connecting - connected in 1 until px(40.dp).toInt())
    }

    @Test fun aLostLinkIsAnnouncedWithoutItsCountdown() {
        val notice = mutableStateOf<ConnectionNotice?>(ConnectionNotice("Reconnecting in 15 s", "Connection refused", true))
        content { CompositionLocalProvider(LocalConnectionNotice provides notice) { ChatHeader() } }
        fun line() = compose.onNode(hasTestTag(CONNECTION_LINE_TAG), useUnmergedTree = true).fetchSemanticsNode()
        assertEquals(LiveRegionMode.Polite, line().config[SemanticsProperties.LiveRegion])
        assertEquals(listOf("Not connected to the host: Connection refused. Reconnecting."), line().descriptions())
        // The drawn text, countdown included, is not part of what is announced.
        assertNull(line().config.getOrNull(SemanticsProperties.Text))
        change { notice.value = ConnectionNotice("Reconnecting in 14 s", "Connection refused", true) }
        settle()
        assertEquals(listOf("Not connected to the host: Connection refused. Reconnecting."), line().descriptions())
    }

    @Test fun theNoticeFollowsTheLink() {
        var link by mutableStateOf<Link>(Link.Connecting)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val notice = rememberConnectionNotice(link)
            Text(notice.value?.text ?: "connected", Modifier.testTag("notice"))
        }
        fun shown() = compose.onNode(hasTestTag("notice")).fetchSemanticsNode().texts().single()
        fun advance(millis: Long) {
            compose.mainClock.advanceTimeBy(millis)
            ShadowLooper.idleMainLooper()
        }
        fun becomes(next: Link) {
            change { link = next }
            advance(50)
        }
        // A first connection is given a moment before it is mentioned at all.
        advance(300)
        assertEquals("connected", shown())
        advance(600)
        assertEquals("Connecting…", shown())
        becomes(Link.Up("host"))
        assertEquals("connected", shown())
        // A lost link says why at once, and counts down to the next attempt.
        becomes(Link.Down("Connection refused", 5_000))
        assertEquals("Reconnecting in 5 s · Connection refused", shown())
        advance(2_000)
        assertEquals("Reconnecting in 3 s · Connection refused", shown())
        // The attempt itself keeps the reason, with no quiet moment.
        becomes(Link.Connecting)
        assertEquals("Reconnecting… · Connection refused", shown())
        becomes(Link.Up("host"))
        assertEquals("connected", shown())
        // Connected since, so the next connection is a quiet one again.
        becomes(Link.Connecting)
        advance(300)
        assertEquals("connected", shown())
    }

    @Test fun everyPageShowsTheLineWhileTheHostIsOutOfReach() = app(link = Link.Down("Connection refused", 15_000)) {
        fun line() = compose.onAllNodes(hasTestTag(CONNECTION_LINE_TAG), useUnmergedTree = true).fetchSemanticsNodes()
        assertEquals(1, line().size)
        // The dot is for a working link only.
        compose.onNode(hasContentDescription("Connected")).assertDoesNotExist()
        open("Reconnect backoff")
        assertEquals("the chat header has no line", 1, line().size)
        assertEquals(
            listOf("Not connected to the host: Connection refused. Reconnecting."),
            line().single().descriptions(),
        )
    }

    // --- Forgetting the host ---

    @Test fun forgettingTheHostAsksFirst() = app {
        compose.onAllNodes(hasContentDescription("Settings"), useUnmergedTree = true).onFirst().performClick()
        idle()
        compose.onAllNodes(hasText("Devices")).onFirst().performClick()
        idle()
        compose.onNode(hasText("Forget this host")).assertExists()
        compose.onNode(hasText("Disconnect")).assertDoesNotExist()
        compose.onNode(hasText("Forget…")).performClick()
        idle()
        compose.onNode(hasText("Forget this host?")).assertExists()
        compose.onNode(hasText("This device will need a new pairing code to connect again.")).assertExists()
        assertEquals("nothing is forgotten before the answer", 0, checkNotNull(host).forgotten.get())
        compose.onNode(hasText("Cancel")).performClick()
        idle()
        compose.onNode(hasText("Forget this host?")).assertDoesNotExist()
        assertEquals(0, checkNotNull(host).forgotten.get())

        compose.onNode(hasText("Forget…")).performClick()
        idle()
        compose.onNode(hasText("Forget") and hasClickAction()).performClick()
        await { checkNotNull(host).forgotten.get() == 1 }
    }

    @Test fun aFailureThatCanBeRetriedOffersTryAgainAndAsksBeforeForgetting() {
        var retried = 0
        var forgotten = 0
        content {
            FatalScreen(
                Link.Fatal(FatalReason.PROTOCOL, "Host sent invalid protocol data"),
                operationError = null,
                retry = { retried++ },
                forget = { forgotten++ },
            )
        }
        compose.onNode(hasText("Try again")).performClick()
        idle()
        assertEquals(1, retried)
        compose.onNode(hasText("Forget this host")).performClick()
        idle()
        compose.onNode(hasText("Forget this host?")).assertExists()
        assertEquals(0, forgotten)
        compose.onNode(hasText("Forget") and hasClickAction()).performClick()
        idle()
        assertEquals(1, forgotten)
    }

    @Test fun aFailureThatNeedsANewPairingOffersOnlyThat() {
        var forgotten = 0
        content {
            FatalScreen(
                Link.Fatal(FatalReason.HOST_KEY_MISMATCH, "host key changed"),
                operationError = null,
                retry = {},
                forget = { forgotten++ },
            )
        }
        compose.onNode(hasText("Try again")).assertDoesNotExist()
        compose.onNode(hasText("Forget this host")).performClick()
        idle()
        // The pairing is already useless, so there is nothing to ask.
        assertEquals(1, forgotten)
    }

    // --- Disclosure rows ---

    @Test fun aDisclosureRowIsAButtonThatSaysItsState() {
        var expanded by mutableStateOf(false)
        content {
            Row(Modifier.testTag("row").disclosure(expanded) { expanded = !expanded }.heightIn(min = 48.dp)) {
                Text("Analysis")
                DisclosureChevron(expanded)
            }
        }
        fun row() = compose.onNode(hasTestTag("row")).fetchSemanticsNode()
        assertEquals(Role.Button, row().config[SemanticsProperties.Role])
        assertEquals("Collapsed", row().config[SemanticsProperties.StateDescription])
        assertEquals("Expand", row().config[SemanticsActions.OnClick].label)
        // The chevron is told by the row's state, not by a name of its own.
        assertEquals(emptyList<String>(), row().descriptions())
        compose.onNode(hasTestTag("row")).performClick()
        settle()
        assertEquals("Expanded", row().config[SemanticsProperties.StateDescription])
        assertEquals("Collapse", row().config[SemanticsActions.OnClick].label)
    }

    @Test fun transcriptRowsThatOpenAreFullSizeButtons() = chat {
        fun row(text: String) = compose.onNode(hasText(text, substring = true) and hasClickAction()).fetchSemanticsNode()
        val group = row("3 tool calls")
        assertEquals(Role.Button, group.config[SemanticsProperties.Role])
        assertEquals("Collapsed", group.config[SemanticsProperties.StateDescription])
        assertTrue("${group.size.height}px tall", group.size.height >= px(48.dp) - 1)
        compose.onNode(hasText("3 tool calls")).performClick()
        idle()
        assertEquals("Expanded", row("3 tool calls").config[SemanticsProperties.StateDescription])
        // A call inside the run, with output of its own to open.
        val nested = row("Grep \"reconnect\"")
        assertEquals("Collapsed", nested.config[SemanticsProperties.StateDescription])
        assertTrue("${nested.size.height}px tall", nested.size.height >= px(48.dp) - 1)
        // A lone call on its own card too.
        val single = row("./gradlew --offline")
        assertTrue("${single.size.height}px tall", single.size.height >= px(48.dp) - 1)
        compose.onAllNodes(hasContentDescription("Expand") or hasContentDescription("Collapse"), useUnmergedTree = true)
            .fetchSemanticsNodes().let { assertEquals("chevrons carry no name", 0, it.size) }
        // A Markdown heading is a heading to a screen reader.
        assertTrue(
            compose.onNode(hasText("Result"), useUnmergedTree = true).fetchSemanticsNode()
                .config.contains(SemanticsProperties.Heading),
        )
    }

    // --- The composer ---

    @Test fun thePhoneComposerIsOneRowOfFullTouchTargets() = chat {
        await { compose.onAllNodes(hasText("Opus 5.5")).fetchSemanticsNodes().isNotEmpty() }
        val attach = compose.onNode(hasContentDescription("Attach images")).fetchSemanticsNode()
        val model = compose.onNode(hasText("Opus 5.5") and hasClickAction()).fetchSemanticsNode()
        val send = compose.onNode(hasContentDescription("Send")).fetchSemanticsNode()
        val field = compose.onNode(hasSetTextAction()).fetchSemanticsNode()
        val targets = listOf("attach" to attach, "model" to model, "send" to send)
        for ((name, node) in targets) {
            val touch = node.touchBoundsInRoot
            assertTrue("$name is ${touch.width}x${touch.height}px", touch.width >= px(48.dp) - 1 && touch.height >= px(48.dp) - 1)
        }
        // No target reaches into its neighbour's, the message field included.
        val all = targets.map { it.first to it.second.touchBoundsInRoot } + ("field" to field.boundsInRoot)
        for ((i, a) in all.withIndex()) for (b in all.drop(i + 1)) {
            assertTrue("${a.first} ${a.second} overlaps ${b.first} ${b.second}", !a.second.inset(0.5f).overlaps(b.second))
        }
        // One row: every control on the same line.
        val centres = targets.map { it.second.touchBoundsInRoot.center.y }
        assertTrue("controls sit at $centres", centres.max() - centres.min() <= 2f)
    }

    @Test fun aChatThatDidNotLoadCanBeTriedAgain() {
        var failing = true
        val scripted = AuditHost { op, _ ->
            if (op == "chat" && failing) """{"ok":false,"error":"The host is restarting."}""" else null
        }
        chat(scripted, await = "The host is restarting.") {
            val error = compose.onNode(hasText("The host is restarting."), useUnmergedTree = true).fetchSemanticsNode()
            assertEquals(LiveRegionMode.Polite, error.config[SemanticsProperties.LiveRegion])
            failing = false
            compose.onNode(hasText("Try again")).performClick()
            await { compose.onAllNodes(hasText("3 tool calls")).fetchSemanticsNodes().isNotEmpty() }
            compose.onNode(hasText("The host is restarting.")).assertDoesNotExist()
            compose.onNode(hasText("Try again")).assertDoesNotExist()
        }
    }

    // --- The Files pane ---

    @Test fun filesRowsAndTabsAreFullSizeAndSayWhatTheyAre() {
        val scripted = AuditHost().connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val files = FilesViewModel { session }
        content { FilesPaneContent(files) }
        await { compose.onAllNodes(hasText("mobile")).fetchSemanticsNodes().isNotEmpty() }
        fun row(name: String) = compose.onNode(hasText(name) and hasClickAction()).fetchSemanticsNode()
        assertEquals("Collapsed", row("mobile").config[SemanticsProperties.StateDescription])
        assertEquals("Open folder", row("mobile").config[SemanticsActions.OnClick].label)
        assertTrue(row("mobile").size.height >= px(48.dp) - 1)
        // A file is not something that folds.
        assertNull(row("README.md").config.getOrNull(SemanticsProperties.StateDescription))
        assertTrue(row("README.md").size.height >= px(48.dp) - 1)
        compose.onNode(hasText("mobile")).performClick()
        await { compose.onAllNodes(hasText("androidApp")).fetchSemanticsNodes().isNotEmpty() }
        assertEquals("Expanded", row("mobile").config[SemanticsProperties.StateDescription])

        compose.onNode(hasText("README.md")).performClick()
        await { files.state.value.open.files.isNotEmpty() }
        idle()
        val close = compose.onNode(hasContentDescription("Close README.md")).fetchSemanticsNode()
        assertEquals(Role.Button, close.config[SemanticsProperties.Role])
        val touch = close.touchBoundsInRoot
        assertTrue("close is ${touch.width}x${touch.height}px", touch.width >= px(48.dp) - 1 && touch.height >= px(48.dp) - 1)
        val tab = compose.onNode(
            hasContentDescription("README.md") and SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Tab),
        ).fetchSemanticsNode()
        assertEquals(true, tab.config[SemanticsProperties.Selected])
        assertTrue(tab.size.height >= px(48.dp) - 1)
        assertTrue("the tab and its close button overlap", !tab.boundsInRoot.inset(0.5f).overlaps(touch))
    }

    // --- The terminal ---

    @Test fun theTerminalIsNamedReadableAndAnnounced() {
        val scripted = AuditHost().connect().also { host = it }
        val terminal = TerminalViewModel.ShellFactory(scripted.client, "global:mobile").create(TerminalViewModel::class.java)
        content { TerminalPaneContent(terminal, showSessionBar = false) }
        await { terminal.state.value.id != null && terminal.state.value.rows.isNotEmpty() }
        idle()
        fun node(tag: String) = compose.onNode(hasTestTag(tag), useUnmergedTree = true).fetchSemanticsNode()

        val surface = node(TERMINAL_SURFACE_TAG)
        assertEquals(listOf("Terminal"), surface.descriptions())
        assertEquals("Show keyboard", surface.config[SemanticsActions.OnClick].label)

        // The screen is one piece of text, not a stop per row.
        val screen = node(TERMINAL_SCREEN_TAG).texts().single()
        assertTrue(screen, "BUILD SUCCESSFUL in 41s" in screen && "git status --short" in screen)
        assertEquals("rows are not stops of their own", 0, compose.onAllNodes(hasText("BUILD SUCCESSFUL in 41s", substring = true)).fetchSemanticsNodes().count { it.id != node(TERMINAL_SCREEN_TAG).id })

        // The line the cursor is on is a polite live region.
        assertEquals(LiveRegionMode.Polite, node(TERMINAL_LIVE_TAG).config[SemanticsProperties.LiveRegion])
        assertEquals(listOf("dev@host:~/xd$"), node(TERMINAL_LIVE_TAG).descriptions())
        val output = Base64.encodeToString("make\r\ncompiling backoff".encodeToByteArray(), Base64.NO_WRAP)
        scripted.push("""{"event":"terminal-output","chat":"global:mobile","terminal":"t1","data":"$output"}""")
        await { node(TERMINAL_LIVE_TAG).descriptions() == listOf("compiling backoff") }

        // The hidden field and the keys a keyboard lacks have names.
        compose.onNode(hasContentDescription("Terminal input") and hasSetTextAction()).assertExists()
        for (key in listOf("Escape", "Tab", "Up", "Down", "Left", "Right")) {
            compose.onNode(hasContentDescription(key)).assertExists()
        }
    }

    @Test fun theTerminalPaletteFollowsTheTheme() {
        val provided = mutableMapOf<MinimalThemePreset, List<androidx.compose.ui.graphics.Color>>()
        compose.setContent {
            for (theme in MinimalThemePreset.entries) {
                XdTheme(theme) { provided[theme] = LocalXdTerminalPalette.current }
            }
        }
        compose.waitForIdle()
        for (theme in MinimalThemePreset.entries) assertEquals(terminalPalette(theme), provided[theme])
    }

    // --- Helpers ---

    /** The phone chat's header as the chat lays it out: its one bar, then the connection line. */
    @Composable
    private fun ChatHeader() {
        Column(Modifier.fillMaxWidth().testTag("header")) {
            CompactChatBar(
                title = "Fix reconnect backoff",
                subtitle = "reconnect-backoff · xd",
                backend = "claude",
                pullRequest = null,
                openPullRequest = {},
                pane = Pane.CHAT,
                onPane = {},
                onBack = {},
                onChatSettings = {},
                chats = SessionChats(emptyList(), null, {}, {}, {}, {}, {}),
            )
            ConnectionLine()
        }
    }

    private fun content(content: @Composable () -> Unit) {
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(MinimalThemePreset.DARK) { Box { content() } } }
        settle()
    }

    /** The whole app against the scripted host, on Projects. */
    private fun app(link: Link? = null, steps: () -> Unit) {
        val activity = compose.activity
        val scripted = AuditHost().connect().also { host = it }
        val model = MainViewModel(activity.application, scripted.client)
        val settings = MobileSettings(activity)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val live by model.client.link.collectAsStateWithLifecycle()
            XdRoot(MinimalThemePreset.DARK) { MinimalMobileApp(model, link ?: live, settings) }
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        scripted.pushSignals()
        await { "c8" in model.client.waitingChats.value && "c9" in model.client.unreadChats.value }
        idle()
        steps()
    }

    /** The phone chat screen on the scripted host's chat with tool calls. */
    private fun chat(scripted: AuditHost = AuditHost(), await: String = "3 tool calls", steps: () -> Unit) {
        host = scripted.connect()
        val model = ChatViewModel(scripted.client, "c1")
        val chats = listOf(ChatSummary("c1", "xd", "Fix reconnect backoff", "claude", false, sessionId = "s1"))
        content {
            ChatScreen(
                model = model,
                goBack = {},
                sessionTitle = "Reconnect backoff",
                projectName = "xd",
                sessionChats = SessionChats(chats, "c1", {}, {}, {}, {}, {}),
            )
        }
        await { compose.onAllNodes(hasText(await)).fetchSemanticsNodes().isNotEmpty() }
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

    private fun px(size: androidx.compose.ui.unit.Dp): Float = with(compose.density) { size.toPx() }

    private fun SemanticsNode.descriptions(): List<String> =
        config.getOrNull(SemanticsProperties.ContentDescription).orEmpty()

    /** What a row says besides its texts and its assistant's name: its status. */
    private fun SemanticsNode.status(): List<String> = descriptions() - setOf("Claude Code", "Codex")

    /** Changes state the composition reads, as an event from outside it would. */
    private fun change(block: () -> Unit) {
        compose.runOnUiThread {
            block()
            androidx.compose.runtime.snapshots.Snapshot.sendApplyNotifications()
        }
    }

    private fun SemanticsNode.texts(): List<String> =
        config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text }

    private fun Rect.inset(by: Float): Rect = Rect(left + by, top + by, right - by, bottom - by)
}
