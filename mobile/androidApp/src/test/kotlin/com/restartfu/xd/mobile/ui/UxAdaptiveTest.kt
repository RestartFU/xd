package com.restartfu.xd.mobile.ui

import android.content.ClipboardManager
import android.content.Context
import android.view.View
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.pinch
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.unit.dp
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.R
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper
import org.robolectric.shadows.ShadowToast

/**
 * Which layout each window gets, that a tool opens beside the chat on a wide
 * one and Back closes it first, that forms and questions outlive a rotation,
 * that a streamed reply ends as it should read, and the smaller conformance
 * points: no toasts, the launch window's ground, the picture viewer.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxAdaptiveTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null

    @After fun closeHost() {
        host?.close()
    }

    // --- Which layout a window gets ---

    @Test fun aPhoneUprightKeepsItsOneHeaderRow() = app {
        openChat()
        compose.onNode(hasContentDescription("Workspace view: Chat")).assertExists()
        // No rail, and the floating bar is away while a chat is open.
        assertFalse(shows("Usage"))
        assertFalse(shows("All chats"))
    }

    @Test @Config(qualifiers = LANDSCAPE_PHONE)
    fun aPhoneOnItsSideKeepsTheCompactChat() = app {
        openChat()
        // The rail at the side, which costs no height, and the phone's one header row.
        assertTrue(shows("Usage"))
        compose.onNode(hasContentDescription("Workspace view: Chat")).assertExists()
        // None of the tablet's stacked rows: no chat tabs, no row of views.
        assertFalse(shows("All chats"))
        assertFalse(shows("Changes"))
        // So the conversation has most of a window only 360dp tall.
        val list = transcript()
        assertTrue("the conversation is ${list.height}px of ${root().height}px", list.height > root().height * 0.5f)
    }

    @Test @Config(qualifiers = UPRIGHT_TABLET)
    fun anUprightTabletKeepsItsTabsAndShowsOneViewAtATime() = app {
        openChat()
        assertTrue(shows("All chats"))
        compose.onNode(hasContentDescription("Workspace view: Chat")).assertDoesNotExist()
        // The views are tabs: one takes the chat's place.
        compose.onNode(hasText("Files") and role(Role.Tab)).assertExists()
        click("Files")
        await { shows("mobile") }
        assertFalse(shows(LAST_LINE))
        compose.onNode(hasTestTag(TOOL_PANE_TAG)).assertDoesNotExist()
    }

    @Test @Config(qualifiers = TABLET)
    fun aWideTabletOpensAToolBesideTheChat() = app { toolOpensBesideTheChat() }

    @Test @Config(qualifiers = SMALL_TABLET)
    fun theNarrowestTwoPaneWindowSplitsEvenlyUnderTwoHeaderRows() = app {
        toolOpensBesideTheChat()
        toggle("Files")
        await { exists(hasTestTag(TOOL_PANE_TAG)) }
        // Title and chat tabs, and no third row of views in a window 600dp tall.
        assertTrue("the header ends at ${transcript().top}px", transcript().top <= px(132.dp))
        val tool = compose.onNode(hasTestTag(TOOL_PANE_TAG)).fetchSemanticsNode().boundsInRoot
        assertEquals(transcript().width, tool.width, px(2.dp))
        // The reader was at the end of the chat, and still is now that its lines are shorter.
        await { endOnScreen(LAST_LINE) }
    }

    private fun toolOpensBesideTheChat() {
        openChat()
        // The views are switches in the header, not a row of tabs under it.
        compose.onNode(hasText("Files") and role(Role.Tab)).assertDoesNotExist()
        val before = transcript()
        toggle("Files")
        await { shows("mobile") }
        // Both at once: the conversation, and the tool to its right.
        assertTrue(shows(LAST_LINE))
        val chat = transcript()
        val tool = compose.onNode(hasTestTag(TOOL_PANE_TAG)).fetchSemanticsNode().boundsInRoot
        assertTrue("the tool starts at ${tool.left}px, the chat ends at ${chat.right}px", tool.left >= chat.right)
        assertTrue(chat.width < before.width)
        assertTrue(tool.width >= px(300.dp) && chat.width >= px(300.dp))
        // The composer stays under the conversation, not under the tool.
        val composer = compose.onNode(hasContentDescription("Send")).fetchSemanticsNode().boundsInRoot
        assertTrue(composer.right <= tool.left)
        // Switched off again, the chat has the window back.
        toggle("Files")
        await { !exists(hasTestTag(TOOL_PANE_TAG)) }
        assertEquals(before.width, transcript().width, 1f)
    }

    // --- Back, with a tool beside the chat ---

    @Test @Config(qualifiers = TABLET)
    fun backClosesTheToolBeforeItLeavesTheSession() = app {
        openChat()
        toggle("Terminal")
        await { exists(hasTestTag(TOOL_PANE_TAG)) }
        screen().pressBack()
        // The tool went; the chat is still open.
        await { !exists(hasTestTag(TOOL_PANE_TAG)) }
        assertTrue(shows(LAST_LINE))
        assertTrue(shows("Back"))
        screen().pressBack()
        await { !shows("Back") }
        assertTrue(shows("Reconnect backoff"))
    }

    // --- What a rotation keeps ---

    @Test @Config(qualifiers = TABLET)
    fun theToolBesideTheChatSurvivesRecreation() = app {
        openChat()
        toggle("Files")
        await { shows("mobile") }
        recreate()
        await { exists(hasTestTag(TOOL_PANE_TAG)) && shows("mobile") }
        assertTrue(shows(LAST_LINE))
    }

    @Test fun aDeleteConfirmationSurvivesRecreation() = app {
        click("Reconnect backoff actions")
        idle()
        click("Delete session")
        await { shows("Delete session?") }
        recreate()
        await { shows("Delete session?") }
        // And still names what it would delete.
        assertTrue(labels().any { "Reconnect backoff" in it && "permanently removes" in it })
    }

    @Test fun thePairingFormSurvivesRecreation() {
        val scripted = AuditHost().also { host = it }
        val model = MainViewModel(compose.activity.application, scripted.client)
        compose.mainClock.autoAdvance = false
        launch { ConnectScreen(model) }
        settle()
        setText(PAIRING_ADDRESS_TAG, "192.168.1.20:7422")
        setText(PAIRING_CODE_TAG, "xd-pair-4f7c-9a21")
        settle()
        recreate()
        await { exists(hasText("192.168.1.20:7422")) }
        compose.onNode(hasText("xd-pair-4f7c-9a21")).assertExists()
    }

    @Test fun thePairingFieldsAskForTheRightKeyboard() {
        val scripted = AuditHost().also { host = it }
        val model = MainViewModel(compose.activity.application, scripted.client)
        compose.mainClock.autoAdvance = false
        launch { ConnectScreen(model) }
        settle()
        fun action(tag: String) = compose.onNode(hasTestTag(tag)).fetchSemanticsNode().config[SemanticsProperties.ImeAction]
        // The address moves on to the code; the code pairs.
        assertEquals(ImeAction.Next, action(PAIRING_ADDRESS_TAG))
        assertEquals(ImeAction.Done, action(PAIRING_CODE_TAG))
    }

    // --- A streamed reply ---

    @Test fun aStreamedReplyEndsAsItsMarkdownReads() {
        var streaming = false
        val scripted = AuditHost(override = { op, request ->
            if (streaming && op == "chat" && request.text("chat") == "c1") STREAMING_CHAT else null
        })
        app(scripted) {
            openChat()
            streaming = true
            scripted.push("""{"event":"turn-started","chat":"c1","turn_id":1,"turn_sequence":1}""")
            idle()
            // In small pieces, faster than they are drawn, as an agent writes.
            STREAMED.chunked(7).forEachIndexed { index, piece ->
                scripted.push(
                    """{"event":"text","chat":"c1","turn_id":1,"turn_sequence":${index + 2},"text":${JsonPrimitive(piece)}}""",
                )
            }
            await { shows("That is the whole change.") }
            idle()
            // Each piece of text drawn on its own: a message is one merged
            // stop for a screen reader, so its blocks are read unmerged.
            val drawn = drawnTexts()
            // Each block as Markdown draws it, none left as its source.
            assertTrue(drawn.toString(), "What changed" in drawn)
            assertTrue("Backoff starts at 2 s and doubles." in drawn)
            assertTrue("replace the fixed timer" in drawn)
            assertTrue("fun next(): Long = 2_000L shl attempt" in drawn)
            assertTrue(drawn.toString(), drawn.any { it.trim() == "64 s" })
            assertFalse(drawn.any { "## " in it || "```" in it || "**" in it })
            // The reply is still streaming: this is the live row, not a stored one.
            assertTrue(scripted.client.openChat("c1").use { it.state.value.working })
        }
    }

    // --- Conformance ---

    @Test fun aLinkNoAppCanOpenIsCopiedAndSaidWithoutAToast() {
        val said = mutableListOf<String>()
        var open: (String) -> Unit = {}
        val refusing = object : UriHandler {
            override fun openUri(uri: String) = throw IllegalArgumentException("No app for $uri")
        }
        compose.mainClock.autoAdvance = false
        launch {
            CompositionLocalProvider(LocalUriHandler provides refusing, LocalNotice provides { said += it }) {
                open = rememberLinkOpener()
            }
        }
        settle()
        compose.runOnUiThread { open("https://github.com/restartfu/xd/pull/212") }
        assertEquals(listOf(LINK_COPIED_NOTICE), said)
        assertNull("no system toast", ShadowToast.getLatestToast())
        val clipboard = compose.activity.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        assertEquals("https://github.com/restartfu/xd/pull/212", clipboard.primaryClip?.getItemAt(0)?.text.toString())
    }

    @Test fun thePictureViewerZoomsWithAPinchAndSaysHowFar() {
        var closed = false
        compose.mainClock.autoAdvance = false
        launch { ImageViewerContent(ImageBitmap(320, 180), "Image curve.png") { closed = true } }
        settle()
        fun state() = compose.onNode(hasTestTag(IMAGE_VIEWER_TAG)).fetchSemanticsNode()
            .config[SemanticsProperties.StateDescription]
        assertEquals("Fitted to the screen", state())
        compose.onNode(hasTestTag(IMAGE_VIEWER_TAG)).performTouchInput {
            pinch(center - Offset(40f, 0f), center - Offset(200f, 0f), center + Offset(40f, 0f), center + Offset(200f, 0f))
        }
        settle()
        assertTrue(state(), state().startsWith("Zoomed to"))
        // Without a pinch, too: a screen reader's actions zoom it.
        val actions = compose.onNode(hasTestTag(IMAGE_VIEWER_TAG)).fetchSemanticsNode()
            .config[SemanticsActions.CustomActions]
        assertEquals(listOf("Zoom in", "Zoom out"), actions.map { it.label })
        compose.onNode(hasContentDescription("Close")).performClick()
        assertTrue(closed)
    }

    @Test fun theLaunchWindowWearsLightsBackdropByDay() = launchWindowMatches(MinimalThemePreset.LIGHT)

    @Test @Config(qualifiers = "+night")
    fun theLaunchWindowWearsDarksBackdropAtNight() = launchWindowMatches(MinimalThemePreset.DARK)

    private fun launchWindowMatches(theme: MinimalThemePreset) {
        val activity = compose.activity
        assertEquals(xdGlass(theme).backdrop.toArgb(), activity.getColor(R.color.xd_ground))
        assertEquals(theme.isLight, activity.resources.getBoolean(R.bool.xd_ground_is_light))
    }

    @Test fun aCutoutAtTheSidePushesThePageClearOfIt() = app {
        fun title() = compose.onNode(hasText("Projects") and SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading))
            .fetchSemanticsNode().boundsInRoot.left
        val before = title()
        val cutout = WindowInsetsCompat.Builder()
            .setInsets(WindowInsetsCompat.Type.displayCutout(), Insets.of(120, 0, 0, 0))
            .build()
        compose.runOnUiThread {
            ViewCompat.dispatchApplyWindowInsets(compose.activity.findViewById<View>(android.R.id.content), cutout)
        }
        idle()
        assertEquals(before + 120f, title(), 1f)
    }

    // --- Helpers ---

    private var content: (@Composable () -> Unit)? = null

    /** Shows [content] in the activity as it is now; [recreate] shows it again in the next one. */
    private fun launch(content: @Composable () -> Unit) {
        this.content = content
        compose.activityRule.scenario.onActivity { activity ->
            activity.setContent { XdRoot(MinimalThemePreset.DARK) { content() } }
        }
    }

    /** Destroys the activity and makes it again, as a rotation does, keeping what Android keeps. */
    private fun recreate() {
        compose.activityRule.scenario.recreate()
        launch(checkNotNull(content))
        idle()
    }

    private fun app(scripted: AuditHost = AuditHost(), steps: () -> Unit) {
        val connected = scripted.connect().also { host = it }
        val model = MainViewModel(compose.activity.application, connected.client)
        val settings = MobileSettings(compose.activity)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        launch {
            val link by model.client.link.collectAsStateWithLifecycle()
            MinimalMobileApp(model, link, settings)
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        connected.pushSignals()
        await { "c8" in model.client.waitingChats.value && "c9" in model.client.unreadChats.value }
        idle()
        steps()
    }

    private fun screen() = ProjectsScreenshotTest.Screen(compose, compose.activity)

    private fun openChat() {
        click("Reconnect backoff")
        await { shows(LAST_LINE) }
        idle()
    }

    private fun matching(label: String) = compose.onAllNodes(
        hasText(label) or hasContentDescription(label),
        useUnmergedTree = true,
    )

    private fun shows(label: String) = matching(label).fetchSemanticsNodes().isNotEmpty()

    private fun exists(matcher: SemanticsMatcher) =
        compose.onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun click(label: String) {
        matching(label).onLast().performClick()
        settle()
    }

    private fun labels(): List<String> = screen().labels()

    /** Every text drawn, one entry per text, without merging. */
    private fun drawnTexts(): List<String> =
        compose.onAllNodes(androidx.compose.ui.test.isRoot(), useUnmergedTree = true).fetchSemanticsNodes().flatMap { root ->
            fun walk(node: androidx.compose.ui.semantics.SemanticsNode): List<androidx.compose.ui.semantics.SemanticsNode> =
                listOf(node) + node.children.flatMap(::walk)
            walk(root)
        }.mapNotNull { node -> node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text } }

    private fun role(role: Role) = SemanticsMatcher.expectValue(SemanticsProperties.Role, role)

    private fun setText(tag: String, text: String) {
        compose.onNode(hasTestTag(tag)).performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(text)) }
    }

    private fun transcript() = compose.onNode(hasTestTag(TRANSCRIPT_TAG)).fetchSemanticsNode().boundsInRoot

    /** Taps a tool's switch in the wide header, which opens it beside the chat or closes it. */
    private fun toggle(tool: String) {
        compose.onNode(hasText(tool) and role(Role.Checkbox)).performClick()
        settle()
    }

    private fun root() = compose.onAllNodes(androidx.compose.ui.test.isRoot())[0].fetchSemanticsNode().boundsInRoot

    /** Whether the text holding [line] ends inside the conversation's viewport. */
    private fun endOnScreen(line: String): Boolean {
        val list = transcript()
        val ends = compose.onAllNodes(hasText(line, substring = true), useUnmergedTree = true)
            .fetchSemanticsNodes().map { it.positionInRoot.y + it.size.height }
        return ends.any { it > list.top && it <= list.bottom + 1f }
    }

    private fun px(size: androidx.compose.ui.unit.Dp): Float = with(compose.density) { size.toPx() }

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
        check(condition()) { "Timed out; showing ${labels()}" }
    }

    private fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content

    private companion object {
        const val LANDSCAPE_PHONE = "w780dp-h360dp-420dpi"
        const val UPRIGHT_TABLET = "w600dp-h960dp-320dpi"
        const val SMALL_TABLET = "w840dp-h600dp-320dpi"
        const val TABLET = "w1280dp-h800dp-320dpi"

        /** The last line of the scripted chat's newest reply. */
        const val LAST_LINE = "Backoff now starts at 2 s and grows to a 2 min cap. All 359 shared tests pass."

        const val STREAMING_CHAT = """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],""" +
            """"plan":false,"turn_id":1,"turn_sequence":1,"segment":"","queue":[],"working":true,"label":"Claude",""" +
            """"items":[],"new_worktree":false,"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""

        val STREAMED = """
            ## What changed

            Backoff starts at **2 s** and doubles.

            - read the connection actor
            - replace the fixed timer

            ```kotlin
            fun next(): Long = 2_000L shl attempt
            ```

            | attempt | delay |
            | --- | --- |
            | 6 | 64 s |

            That is the whole change.
        """.trimIndent()
    }
}
