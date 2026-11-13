package com.restartfu.xd.mobile.ui

import android.provider.Settings
import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToKeyAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.TerminalViewModel
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.PendingInteraction
import com.restartfu.xd.syntax.SyntaxLanguage
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.runBlocking
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
 * The third audit's fixes: switch rows that are one stop, the file editor
 * above the keyboard, terminal keys that are buttons, Usage's choices as
 * radio groups, every text role readable on its grounds, full-size touch
 * targets, approval diffs read off the main thread and capped with their
 * answers in reach, Stop that asks, sessions as lazy rows, Back's gesture
 * driving the page change, caches capped by size, and the open tool kept
 * through the end of the process.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxPolishTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null

    @After fun close() {
        host?.close()
    }

    // --- Switch rows ---

    @Test fun everySwitchInSettingsChatsIsOneStopNamedByItsWords() = app {
        click("Settings")
        await { shows("Chats") }
        click("Chats")
        idle()
        val switches = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.ToggleableState))
            .fetchSemanticsNodes()
        assertTrue("no switches shown", switches.isNotEmpty())
        for (switch in switches) {
            // Named by the row's words, and the whole row is what turns it.
            assertTrue("a switch says nothing: ${switch.config}", switch.texts().isNotEmpty())
            assertEquals(Role.Switch, switch.config.getOrNull(SemanticsProperties.Role))
            assertTrue(switch.config.contains(SemanticsActions.OnClick))
            assertTrue("${switch.texts()} is ${switch.size.height}px tall", switch.size.height >= px(48.dp) - 1)
        }
        val resume = switches.single { node -> node.texts().any { it.startsWith("Resume") } }
        val before = resume.config[SemanticsProperties.ToggleableState]
        // A tap on the words turns it.
        compose.onNode(hasText(resume.texts().first()), useUnmergedTree = true).performClick()
        idle()
        val after = compose.onNode(hasText(resume.texts().first()) and hasClickAction()).fetchSemanticsNode()
            .config[SemanticsProperties.ToggleableState]
        assertNotEquals(before, after)
    }

    // --- The keyboard ---

    @Test fun theFileEditorsSaveStaysJustAboveTheKeyboard() = chat {
        click("Workspace view: Chat")
        click("Files")
        await { shows("README.md") }
        click("README.md")
        await { exists(hasText("Save") and hasClickAction()) }
        showKeyboard(compose.activity, px(KEYBOARD).toInt(), px(NAVIGATION).toInt())
        idle()
        val root = compose.onNode(isRoot()).fetchSemanticsNode().boundsInRoot
        val keyboardTop = root.bottom - px(KEYBOARD) - px(NAVIGATION)
        val save = bottomOf(hasText("Save") and hasClickAction())
        assertTrue("Save ends at $save, under the keyboard at $keyboardTop", save <= keyboardTop + 1)
        // And no higher than its row's own padding: the navigation bar is not counted twice.
        assertTrue("Save ends at $save, ${keyboardTop - save}px over the keyboard", save >= keyboardTop - px(12.dp))
    }

    @Test fun theTerminalsKeysSitJustAboveTheKeyboard() = chat {
        click("Workspace view: Chat")
        click("Terminal")
        await { exists(hasContentDescription("Escape")) }
        showKeyboard(compose.activity, px(KEYBOARD).toInt(), px(NAVIGATION).toInt())
        idle()
        val root = compose.onNode(isRoot()).fetchSemanticsNode().boundsInRoot
        val keyboardTop = root.bottom - px(KEYBOARD) - px(NAVIGATION)
        val key = bottomOf(hasContentDescription("Escape"))
        assertTrue("the keys end at $key, under the keyboard at $keyboardTop", key <= keyboardTop + 1)
        assertTrue("the keys end ${keyboardTop - key}px over the keyboard", key >= keyboardTop - px(12.dp))
    }

    // --- Terminal keys ---

    @Test fun terminalKeysAreButtonsAndCtrlIsASwitch() = terminal {
        for (name in listOf("Escape", "Tab", "Up", "Down", "Left", "Right")) {
            val key = compose.onNode(hasContentDescription(name)).fetchSemanticsNode()
            assertEquals(name, Role.Button, key.config.getOrNull(SemanticsProperties.Role))
            assertTrue(name, !key.config.contains(SemanticsProperties.ToggleableState))
            assertTrue(name, !key.config.contains(SemanticsProperties.Selected))
            val touch = key.touchBoundsInRoot
            assertTrue("$name is ${touch.width}x${touch.height}px", touch.width >= px(48.dp) - 1 && touch.height >= px(48.dp) - 1)
        }
        val ctrl = { compose.onNode(hasContentDescription("Control, for the next key")).fetchSemanticsNode() }
        assertEquals(Role.Switch, ctrl().config.getOrNull(SemanticsProperties.Role))
        assertEquals(ToggleableState.Off, ctrl().config[SemanticsProperties.ToggleableState])
        compose.onNode(hasContentDescription("Control, for the next key")).performClick()
        idle()
        assertEquals(ToggleableState.On, ctrl().config[SemanticsProperties.ToggleableState])
    }

    // --- Stop ---

    @Test fun stoppingTheSharedTerminalAsksFirstAndTheQuestionSurvivesRecreation() {
        val scripted = AuditHost().connect().also { host = it }
        val terminal = TerminalViewModel.ShellFactory(scripted.client, "global:mobile").create(TerminalViewModel::class.java)
        val restoration = StateRestorationTester(compose)
        compose.mainClock.autoAdvance = false
        var made = 0
        restoration.setContent {
            remember { ++made }
            XdRoot(MinimalThemePreset.DARK) { Box { TerminalPaneContent(terminal, showSessionBar = true) } }
        }
        await { terminal.state.value.id != null }
        idle()
        compose.onNode(hasText("Stop") and hasClickAction()).performClick()
        idle()
        await { shows("Stop this terminal?") }
        assertTrue(shows("It closes the shell on every device, and anything running in it."))
        assertEquals("killed on the first tap", 0, sent("terminal-kill"))
        restore(restoration)
        // Made again from what was saved, not merely left up.
        assertEquals(2, made)
        await { shows("Stop this terminal?") }
        // Cancel leaves it running.
        compose.onNode(hasText("Cancel") and hasAnyAncestor(isDialog())).performClick()
        idle()
        assertTrue(!shows("Stop this terminal?"))
        assertEquals(0, sent("terminal-kill"))
        compose.onNode(hasText("Stop") and hasClickAction()).performClick()
        idle()
        await { shows("Stop this terminal?") }
        compose.onNode(hasText("Stop") and hasAnyAncestor(isDialog())).performClick()
        idle()
        await { sent("terminal-kill") == 1 }
    }

    // --- Usage ---

    @Test fun usageChoicesAreRadioButtonsInGroups() {
        val scripted = AuditHost().connect().also { host = it }
        val usage = UsageViewModel(scripted.client, MobileSettings(compose.activity))
        content { UsageContent(usage) }
        idle()
        fun choice(label: String) = compose.onNode(hasText(label) and hasClickAction()).fetchSemanticsNode()
        for (label in listOf("Cost", "Tokens", "Limits", "All providers", "Codex", "Claude")) {
            assertEquals(label, Role.RadioButton, choice(label).config.getOrNull(SemanticsProperties.Role))
        }
        // One of each set is chosen, and says so.
        assertEquals(1, listOf("Cost", "Tokens", "Limits").count { choice(it).config[SemanticsProperties.Selected] })
        assertEquals(1, listOf("All providers", "Codex", "Claude").count { choice(it).config[SemanticsProperties.Selected] })
        fun SemanticsNode.holds(label: String): Boolean = label in texts() || children.any { it.holds(label) }
        val groups = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.SelectableGroup), useUnmergedTree = true)
            .fetchSemanticsNodes()
        assertEquals(1, groups.count { it.holds("Cost") && it.holds("Tokens") && it.holds("Limits") && !it.holds("Codex") })
        assertEquals(1, groups.count { it.holds("Codex") && it.holds("Claude") && !it.holds("Cost") })
    }

    // --- Contrast ---

    @Test fun everyTextRoleReadsOnItsGroundsInBothThemes() {
        for (theme in MinimalThemePreset.entries) {
            val scheme = minimalColors(theme)
            val signals = xdSignals(theme)
            val glass = xdGlass(theme)
            // Every surface text is drawn on: the page backdrop, cards, menus,
            // dialogs, the glass panes and composer laid over the backdrop, and
            // the diff rows' tints.
            val grounds = mapOf(
                "background" to scheme.background, "surface" to scheme.surface,
                "surfaceVariant" to scheme.surfaceVariant, "surfaceContainerLowest" to scheme.surfaceContainerLowest,
                "surfaceContainerLow" to scheme.surfaceContainerLow, "surfaceContainer" to scheme.surfaceContainer,
                "surfaceContainerHigh" to scheme.surfaceContainerHigh, "surfaceContainerHighest" to scheme.surfaceContainerHighest,
                "backdrop" to glass.backdrop, "pane" to glass.pane.compositeOver(glass.backdrop),
                "composer" to glass.composer.compositeOver(glass.backdrop),
                "diffAdded" to signals.diffAdded, "diffRemoved" to signals.diffRemoved,
            )
            val texts = mapOf(
                "onSurface" to scheme.onSurface, "onSurfaceVariant" to scheme.onSurfaceVariant,
                "faint" to signals.faint, "primary" to scheme.primary, "error" to scheme.error,
                "link" to signals.link, "working" to signals.working, "waiting" to signals.waiting, "done" to signals.done,
            )
            for ((text, ink) in texts) for ((ground, colour) in grounds) {
                val ratio = contrastRatio(ink, colour)
                assertTrue("$theme $text on $ground is $ratio:1", ratio >= 4.5f)
            }
            // And each container's own text role on it.
            val pairs = mapOf(
                "onPrimary on primary" to (scheme.onPrimary to scheme.primary),
                "onPrimaryContainer on primaryContainer" to (scheme.onPrimaryContainer to scheme.primaryContainer),
                "onSecondaryContainer on secondaryContainer" to (scheme.onSecondaryContainer to scheme.secondaryContainer),
                "onError on error" to (scheme.onError to scheme.error),
                "onErrorContainer on errorContainer" to (scheme.onErrorContainer to scheme.errorContainer),
                "inverseOnSurface on inverseSurface" to (scheme.inverseOnSurface to scheme.inverseSurface),
            )
            for ((name, pair) in pairs) {
                val ratio = contrastRatio(pair.first, pair.second)
                assertTrue("$theme $name is $ratio:1", ratio >= 4.5f)
            }
        }
    }

    @Test fun noTextIsDrawnInTheOutlineColour() {
        // The outline colour is for strokes; at 3.6 to 4.4:1 in Light it is too faint for words.
        val sources = java.io.File("src/main/kotlin/com/restartfu/xd/mobile").walkTopDown().filter { it.extension == "kt" }.toList()
        assertTrue(sources.isNotEmpty())
        val inText = Regex("""(color|tint|contentColor) = MaterialTheme\.colorScheme\.outline\b(?!Variant)|-> MaterialTheme\.colorScheme\.outline$""", RegexOption.MULTILINE)
        val found = sources.flatMap { file ->
            file.readLines().withIndex().filter { (_, line) -> inText.containsMatchIn(line) }.map { "${file.name}:${it.index + 1}" }
        }
        assertTrue("text in the outline colour at $found", found.isEmpty())
    }

    @Test fun lightsNavigationButtonsReadOnAndroid8() {
        // White buttons over the scrim laid on Light's backdrop, where they cannot be made dark.
        val ground = NavigationBarDarkScrim.compositeOver(xdGlass(MinimalThemePreset.LIGHT).backdrop)
        assertTrue(contrastRatio(Color.White, ground) >= 3f)
        assertEquals(NavigationBarDarkScrim, navigationBarColor(light = true, sdk = 26))
        assertEquals(Color.Transparent, navigationBarColor(light = true, sdk = 27))
        assertEquals(Color.Transparent, navigationBarColor(light = false, sdk = 26))
    }

    // --- Touch targets ---

    @Test fun agentPillsHaveFullTouchTargets() {
        val opened = AtomicInteger()
        val parent = ChatSummary("c0", "xd", "Plan the release", "claude", false, sessionId = "s0")
        val report = com.restartfu.xd.model.AgentReport("c2", "Usage page", com.restartfu.xd.model.AgentOutcome.DONE)
        content {
            Column {
                StartedByPill(parent) { opened.incrementAndGet() }
                AgentReportLine(report, null) { opened.incrementAndGet() }
            }
        }
        for (label in listOf("Plan the release", "Usage page finished")) {
            // Laid out at full size, not only stretched by Compose's touch slop round a smaller node.
            val target = compose.onNode(hasText(label, substring = true) and hasClickAction()).fetchSemanticsNode().size
            assertTrue("$label is ${target.width}x${target.height}px", target.width >= px(48.dp) - 1 && target.height >= px(48.dp) - 1)
        }
    }

    @Test fun everyTargetOnProjectsAChatAndSettingsIsFullSize() = app {
        checkTargets("Projects")
        open("Reconnect backoff")
        checkTargets("the chat")
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        idle()
        click("Settings")
        await { shows("Chats") }
        checkTargets("Settings")
        click("Chats")
        idle()
        checkTargets("Settings › Chats")
    }

    // --- Approvals ---

    @Test fun aLongApprovalDiffIsCappedAndItsAnswersStayInReach() {
        val decided = AtomicInteger()
        val request = PendingInteraction(
            requestId = "r-long", method = "approval", questions = emptyList(),
            detail = "Edit shared/Backoff.kt", tool = "Edit", reason = null, patch = LONG_PATCH,
        )
        val computed = DiffColours.computed.get()
        content { Column(Modifier.fillMaxSize()) { ApprovalAsk(request, 1) { decided.incrementAndGet() } } }
        await { shows("Show all 423 lines") }
        // Read once, in the background.
        assertEquals(computed + 1, DiffColours.computed.get())
        val drawn = compose.onAllNodes(SemanticsMatcher("a diff line") { node ->
            node.texts().any { it.startsWith("+val step") || it.startsWith("-val step") }
        }, useUnmergedTree = true).fetchSemanticsNodes().size
        assertTrue("$drawn lines drawn", drawn in 1..DIFF_INLINE_LINES)
        // The answers sit under the part that scrolls, inside the ask's frame.
        val frame = compose.onNode(hasTestTag(ASK_SCROLL_TAG)).fetchSemanticsNode().boundsInRoot
        val screen = compose.onNode(isRoot()).fetchSemanticsNode().boundsInRoot
        for (answer in listOf("Allow once", "Allow for session", "Deny")) {
            val bounds = compose.onNode(hasText(answer) and hasClickAction()).fetchSemanticsNode().boundsInRoot
            assertTrue("$answer at $bounds, under the patch at $frame", bounds.top >= frame.bottom - 1)
            assertTrue("$answer at $bounds, off a screen of $screen", bounds.bottom <= screen.height / 2 + px(48.dp))
        }
        compose.onNode(hasText("Allow once") and hasClickAction()).performClick()
        idle()
        assertEquals(1, decided.get())
        // Further down the ask than the screen shows, so by its action rather than a tap where it sits.
        compose.onNode(hasText("Show all 423 lines") and hasClickAction()).performSemanticsAction(SemanticsActions.OnClick)
        idle()
        assertTrue(!shows("Show all 423 lines"))
    }

    // --- Lazy sessions ---

    @Test fun eachSessionIsALazyRowOfItsOwn() = app {
        val list = compose.onNode(hasScrollToKeyAction()).fetchSemanticsNode()
        val index = list.config[SemanticsProperties.IndexForKey]
        val project = index("xd")
        val first = index("session-s1")
        val second = index("session-s2")
        // The project's row is its own, and each of its sessions another under it.
        assertTrue("project at $project, sessions at $first and $second", project >= 0 && first > project && second > project)
        assertNotEquals(first, second)
        assertTrue(shows("Reconnect backoff") && shows("Usage page"))
    }

    // --- Back ---

    @Test fun backsGestureBringsProjectsInAsItIsDragged() = app {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
        open("Reconnect backoff")
        assertTrue(!shows("Projects"))
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        // Half way: Projects is coming in, and the chat is still there.
        assertTrue("Projects is not shown while the gesture is held", shows("Projects"))
        assertTrue(shows("3 tool calls"))
        compose.runOnUiThread { dispatcher.dispatchOnBackCancelled() }
        repeat(3) { idle() }
        // Called off: back on the chat alone.
        assertTrue(!shows("Projects"))
        assertTrue(shows("3 tool calls"))
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.6f)) }
        idle()
        compose.runOnUiThread { dispatcher.onBackPressed() }
        repeat(3) { idle() }
        await { !shows("3 tool calls") }
        assertTrue(shows("Projects"))
    }

    @Test fun backsGestureMovesNothingWhileAnimationsAreOff() = app {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        open("Reconnect backoff")
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertTrue(!shows("Projects"))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        idle()
        await { shows("Projects") && !shows("3 tool calls") }
    }

    @Test fun backsGestureFromASettingsSectionBringsTheSettingsHomeIn() = app {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
        click("Settings")
        await { shows("Chats") }
        click("Chats")
        repeat(2) { idle() }
        assertTrue(!shows("Appearance"))
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertTrue("the Settings home is not shown while the gesture is held", shows("Appearance"))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        repeat(3) { idle() }
        // On the Settings home, still in Settings.
        assertTrue(shows("Appearance") && shows("Settings"))
    }

    // --- Caches ---

    @Test fun aPreparedCacheKeepsToItsCharacters() {
        val cache = PreparedCache<CodeKey, String>(capacity = 100, maxWeight = 1_000, weigh = { it.code.length }) { it.code.uppercase() }
        val ink = CodeInk(Color.White, xdSyntax(MinimalThemePreset.DARK))
        val keys = (1..12).map { CodeKey("x".repeat(200) + it, SyntaxLanguage.NONE, ink) }
        runBlocking { keys.forEach { cache.request(it).await() } }
        assertTrue("${cache.totalWeight} characters kept", cache.totalWeight <= 1_000)
        assertTrue(cache.size in 1..4)
        // The newest are the ones kept.
        assertEquals(keys.last().code.uppercase(), cache.peek(keys.last()))
        assertEquals(null, cache.peek(keys.first()))
        // One block bigger than the whole cap is still kept, alone, as it was just asked for.
        val huge = CodeKey("y".repeat(5_000), SyntaxLanguage.NONE, ink)
        runBlocking { cache.request(huge).await() }
        assertEquals(1, cache.size)
        assertEquals(huge.code.uppercase(), cache.peek(huge))
    }

    @Test fun keysWithTheSameHashStillTellTheirTextsApart() {
        val ink = CodeInk(Color.White, xdSyntax(MinimalThemePreset.DARK))
        // "Aa" and "BB" share a String hash code.
        assertEquals("Aa".hashCode(), "BB".hashCode())
        val a = CodeKey("Aa", SyntaxLanguage.NONE, ink)
        val b = CodeKey("BB", SyntaxLanguage.NONE, ink)
        assertNotEquals(a, b)
        assertEquals(a, CodeKey(String(charArrayOf('A', 'a')), SyntaxLanguage.NONE, ink))
        val cache = PreparedCache<CodeKey, String>(capacity = 10) { it.code.lowercase() }
        runBlocking {
            assertEquals("aa", cache.request(a).await())
            assertEquals("bb", cache.request(b).await())
        }
        assertNotEquals(ReplyKey("1", "Aa"), ReplyKey("1", "BB"))
        assertEquals(ReplyKey("1", "Aa"), ReplyKey("1", "Aa"))
    }

    // --- The open tool ---

    @Test fun theOpenToolSurvivesTheEndOfTheProcess() {
        val scripted = AuditHost().connect().also { host = it }
        val model = ChatViewModel(scripted.client, "c1")
        val chats = listOf(ChatSummary("c1", "xd", "Chat", "claude", false, sessionId = "s1"))
        // A new owner after the restore stands for the models a new process makes from nothing.
        var owner: ViewModelStoreOwner = Owner()
        val restoration = StateRestorationTester(compose)
        compose.mainClock.autoAdvance = false
        var made = 0
        restoration.setContent {
            remember { ++made }
            XdRoot(MinimalThemePreset.DARK) {
                Box {
                    ChatScreen(
                        model = model,
                        goBack = {},
                        sessionTitle = "Reconnect backoff",
                        projectName = "xd",
                        workspaceOwner = owner,
                        sessionChats = SessionChats(chats, "c1", {}, {}, {}, {}, {}),
                    )
                }
            }
        }
        await { shows("3 tool calls") }
        idle()
        click("Workspace view: Chat")
        click("Files")
        await { shows("README.md") }
        owner = Owner()
        restore(restoration)
        assertEquals(2, made)
        await { shows("README.md") }
        assertTrue(shows("Workspace view: Files"))
        assertTrue(!shows("3 tool calls"))
    }

    // --- Helpers ---

    /** Saves the content's state, lets it go, and makes it again from what was saved. */
    private fun restore(restoration: StateRestorationTester) {
        // Its steps wait for the content to settle, which the paused test clock never lets it do.
        compose.mainClock.autoAdvance = true
        restoration.emulateSavedInstanceStateRestore()
        compose.mainClock.autoAdvance = false
        idle()
    }

    private class Owner : ViewModelStoreOwner {
        override val viewModelStore = ViewModelStore()
    }

    private fun edge(progress: Float) = BackEventCompat(0f, 400f, progress, BackEventCompat.EDGE_LEFT)

    /** Every node that can be tapped is at least 48dp each way, by its touch bounds. */
    private fun checkTargets(page: String) {
        val small = compose.onAllNodes(hasClickAction()).fetchSemanticsNodes()
            .filter { node ->
                // By its touch bounds, which take in the room Material keeps round a
                // smaller drawn control, as an icon button's 40dp in its 48dp.
                val touch = node.touchBoundsInRoot
                // Laid out but off screen, under a bar, or clipped by a scroll: not a target now.
                node.boundsInRoot.height >= node.size.height - 1 && touch.height > 0f &&
                    (touch.width < px(48.dp) - 1 || touch.height < px(48.dp) - 1)
            }
        assertTrue(
            "small targets on $page: " + small.joinToString { "${it.texts() + it.descriptions()} ${it.touchBoundsInRoot}" },
            small.isEmpty(),
        )
    }

    private fun sent(op: String): Int = checkNotNull(host).sent.count { (it["op"] as? JsonPrimitive)?.content == op }

    private fun bottomOf(matcher: SemanticsMatcher): Float {
        val node = compose.onNode(matcher).fetchSemanticsNode()
        return node.positionInRoot.y + node.size.height
    }

    private fun shows(label: String): Boolean = compose.onAllNodes(
        hasText(label) or hasContentDescription(label),
        useUnmergedTree = true,
    ).fetchSemanticsNodes().isNotEmpty()

    private fun exists(matcher: SemanticsMatcher): Boolean =
        compose.onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun click(label: String) {
        compose.onAllNodes(hasText(label) or hasContentDescription(label), useUnmergedTree = true).onLast().performClick()
        idle()
    }

    private fun open(session: String) {
        compose.onAllNodes(hasText(session), useUnmergedTree = true).onLast().performClick()
        await { compose.onAllNodes(hasText("3 tool calls")).fetchSemanticsNodes().isNotEmpty() }
        idle()
    }

    private fun content(content: @Composable () -> Unit) {
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(MinimalThemePreset.DARK) { Box { content() } } }
        settle()
    }

    private fun app(steps: () -> Unit) {
        val activity = compose.activity
        val scripted = AuditHost().connect().also { host = it }
        val model = MainViewModel(activity.application, scripted.client)
        val settings = MobileSettings(activity)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val live by model.client.link.collectAsStateWithLifecycle()
            XdRoot(MinimalThemePreset.DARK) { MinimalMobileApp(model, live, settings) }
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        idle()
        steps()
    }

    private fun chat(steps: () -> Unit) {
        val scripted = AuditHost().connect().also { host = it }
        val model = ChatViewModel(scripted.client, "c1")
        val chats = listOf(ChatSummary("c1", "xd", "Chat", "claude", false, sessionId = "s1"))
        content {
            ChatScreen(
                model = model,
                goBack = {},
                sessionTitle = "Reconnect backoff",
                projectName = "xd",
                sessionChats = SessionChats(chats, "c1", {}, {}, {}, {}, {}),
            )
        }
        await { shows("3 tool calls") }
        idle()
        steps()
    }

    private fun terminal(steps: () -> Unit) {
        val scripted = AuditHost().connect().also { host = it }
        val terminal = TerminalViewModel.ShellFactory(scripted.client, "global:mobile").create(TerminalViewModel::class.java)
        content { TerminalPaneContent(terminal, showSessionBar = true) }
        await { terminal.state.value.id != null }
        idle()
        steps()
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

    private fun px(size: Dp): Float = with(compose.density) { size.toPx() }

    private fun SemanticsNode.texts(): List<String> =
        config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text }

    private fun SemanticsNode.descriptions(): List<String> =
        config.getOrNull(SemanticsProperties.ContentDescription).orEmpty()

    private companion object {
        val KEYBOARD = 300.dp
        val NAVIGATION = 48.dp

        /** A patch of 420 changed lines in one file, and its three header lines. */
        val LONG_PATCH: String = buildString {
            append("--- a/shared/Backoff.kt\n+++ b/shared/Backoff.kt\n")
            append("@@ -1,210 +1,210 @@\n")
            for (i in 1..210) {
                append("-val step$i = $i * 1_000L\n")
                append("+val step$i = minOf(2_000L shl $i, 120_000L)\n")
            }
        }.trimEnd('\n')
    }
}
