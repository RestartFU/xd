package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.provider.Settings
import android.util.Base64
import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.isSelected
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.AnnotatedString
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.files.FileTree
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.protocol.FileEntryReply
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper
import org.robolectric.Shadows.shadowOf
import android.os.Looper
import java.time.Duration

/**
 * The seventh audit's fixes: Projects marked on the rail while a session is
 * open; selected chat tabs at 3:1; the global Terminal kept through a
 * rotation; predictive Back from an agent tab and a Usage model; the focus
 * back on the row that was opened; a chat named once, not again when the
 * host titles it; fields named inside them; the file tree walked once per
 * tree; and Undo for a dropped queued message and a removed picture.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxSeventhTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null
    private lateinit var model: MainViewModel

    @After fun close() {
        host?.close()
    }

    // --- The rail in a session ---

    @Test @Config(qualifiers = SMALL_TABLET)
    fun theRailMarksProjectsInASessionOnASmallTablet() = railInASession()

    @Test @Config(qualifiers = LANDSCAPE_PHONE)
    fun theRailMarksProjectsInASessionOnAPhoneOnItsSide() = railInASession()

    private fun railInASession() = app {
        openChat()
        assertTrue("Projects is not the rail's current place in a session", railShows(railItem("Projects") and isSelected()))
        assertFalse(railShows(railItem("Terminal") and isSelected()))
        // Its Projects goes back, as Back does.
        compose.onNode(railItem("Projects")).performClick()
        idle()
        await { !shows(CHAT_MARK) }
        assertTrue(railShows(railItem("Projects") and isSelected()))
        assertTrue(shows("Reconnect backoff"))
    }

    // --- The selected tab ---

    @Test @Config(qualifiers = SMALL_TABLET)
    fun theSelectedChatTabStandsOutDark() = selectedTab(MinimalThemePreset.DARK)

    @Test @Config(qualifiers = SMALL_TABLET)
    fun theSelectedChatTabStandsOutLight() = selectedTab(MinimalThemePreset.LIGHT)

    @Test fun theSelectedTabsEdgeIsThreeToOneOnTheHeader() {
        for (theme in MinimalThemePreset.entries) {
            // The tablet header's one pane of glass, laid over the backdrop.
            val ratio = contrastRatio(xdSignals(theme).controlBorder, xdGlass(theme).overlay())
            assertTrue("$theme: the selected tab's edge is $ratio:1 on the header", ratio >= 3f)
        }
    }

    private fun selectedTab(theme: MinimalThemePreset) = app(theme = theme) {
        openChat()
        val tab = compose.onNode(
            hasText("Fix reconnect backoff") and SemanticsMatcher.expectValue(SemanticsProperties.Selected, true),
        ).fetchSemanticsNode().boundsInWindow
        val bitmap = ProjectsScreenshotTest.Screen(compose, compose.activity).capture()
        val y = tab.center.y.toInt()
        // Its edge, against the header's glass just outside it.
        val edge = Color(bitmap.getPixel(tab.left.toInt() + 1, y))
        val header = Color(bitmap.getPixel(tab.left.toInt() - 4, y))
        val ratio = contrastRatio(edge, header)
        assertTrue("$theme: the selected tab's edge is $ratio:1 on the header", ratio >= 3f)
    }

    // --- The global Terminal through a rotation ---

    @Test fun theGlobalTerminalSurvivesARotationWithoutOpeningAgain() = app {
        click("Terminal")
        await { opened() >= 1 }
        repeat(3) { idle() }
        // Ctrl latched, as for Ctrl-C next.
        compose.onNode(hasContentDescription("Control, for the next key")).performClick()
        idle()
        val before = opened()
        recreate()
        repeat(4) { idle() }
        assertEquals("the terminal was asked for again after the rotation", before, opened())
        assertTrue(shows("dev@host", substring = true))
        assertEquals(
            ToggleableState.On,
            compose.onNode(hasContentDescription("Control, for the next key")).fetchSemanticsNode()
                .config[SemanticsProperties.ToggleableState],
        )
        // Leaving the page still lets the shell go: coming back opens it afresh.
        click("Projects")
        repeat(3) { idle() }
        click("Terminal")
        await { opened() > before }
    }

    // --- Predictive Back on the two in-page paths ---

    @Test fun backsGestureFromAnAgentTabBringsTheChatIn() = app(override = withAgent) {
        motion(true)
        openAgent()
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertTrue("the chat is not coming in while the gesture is held", shows(CHAT_MARK))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        repeat(3) { idle() }
        await { shows(CHAT_MARK) && !shows(AGENT_PROMPT) }
    }

    @Test fun backsGestureFromAnAgentTabCutsWhileAnimationsAreOff() = app(override = withAgent) {
        motion(false)
        openAgent()
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertFalse(shows(CHAT_MARK))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        idle()
        await { shows(CHAT_MARK) && !shows(AGENT_PROMPT) }
    }

    @Test fun backsGestureFromAUsageModelBringsTheOverviewIn() = app {
        motion(true)
        openModel()
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertTrue("the overview is not coming in while the gesture is held", shows("Sessions"))
        assertTrue(shows("Back to overview"))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        repeat(3) { idle() }
        await { shows("Sessions") && !shows("Back to overview") }
        // Still on Usage: Back left the model, not the page.
        assertTrue(shows("API estimate"))
    }

    @Test fun backsGestureFromAUsageModelCutsWhileAnimationsAreOff() = app {
        motion(false)
        openModel()
        val dispatcher = compose.activity.onBackPressedDispatcher
        compose.runOnUiThread { dispatcher.dispatchOnBackStarted(edge(0f)) }
        compose.runOnUiThread { dispatcher.dispatchOnBackProgressed(edge(0.5f)) }
        idle()
        assertFalse(shows("Sessions"))
        compose.runOnUiThread { dispatcher.onBackPressed() }
        idle()
        await { shows("Sessions") && !shows("Back to overview") }
    }

    // --- The focus after Back ---

    @Test fun backToProjectsFocusesTheSessionThatWasOpened() = app {
        click("Pairing flow")
        await { !shows("Usage page") }
        idle()
        pressBack()
        await { shows("Usage page") }
        repeat(2) { idle() }
        assertTrue("focus is on ${focusedText()}", focusedText().startsWith("Pairing flow"))
    }

    @Test fun backToProjectsFocusesTheHeadingWhenTheSessionIsGone() {
        val gone = AtomicBoolean(false)
        app(override = { op, _ ->
            if (op == "tree" && gone.get()) withoutSession(AuditHost.TREE, "s3") else null
        }) {
            click("Pairing flow")
            await { !shows("Usage page") }
            idle()
            gone.set(true)
            checkNotNull(host).push("""{"event":"tree"}""")
            await({ "tree: ${model.client.tree.value.chats.map { it.id }}" }) { model.client.tree.value.chats.none { it.id == "c3" } }
            repeat(3) { idle() }
            await { shows("Usage page") }
            repeat(2) { idle() }
            assertEquals("Projects", focusedText())
        }
    }

    @Test fun backToTheSettingsHomeFocusesTheSectionThatWasOpened() = app {
        click("Settings")
        await { shows("Chats") }
        click("Chats")
        repeat(2) { idle() }
        pressBack()
        await { shows("Appearance") }
        repeat(2) { idle() }
        assertTrue("focus is on ${focusedText()}", focusedText().startsWith("Chats"))
    }

    @Test fun backToTheUsageOverviewFocusesTheModelThatWasOpened() = app {
        openModel()
        pressBack()
        await { shows("Sessions") && !shows("Back to overview") }
        repeat(2) { idle() }
        assertTrue("focus is on ${focusedText()}", focusedText().startsWith("gpt-6-luna"))
    }

    // --- A chat named once ---

    @Test fun aChatIsNotAnnouncedAgainWhenTheHostTitlesIt() {
        val titled = AtomicBoolean(false)
        app(override = { op, request ->
            when {
                !titled.get() -> null
                op == "tree" -> AuditHost.TREE.replace("Fix reconnect backoff", RETITLED)
                op == "chat" && request.text("chat") == "c1" -> AuditHost.reply(op, request).replace("Fix reconnect backoff", RETITLED)
                else -> null
            }
        }) {
            openChat()
            assertEquals(listOf("Fix reconnect backoff"), paneTitles().filter { it != "Projects" })
            titled.set(true)
            checkNotNull(host).push("""{"event":"tree"}""")
            checkNotNull(host).push("""{"event":"changed","chat":"c1"}""")
            await { model.client.tree.value.chats.any { it.title == RETITLED } }
            repeat(3) { idle() }
            // The header shows the new title; the page is not said again under it.
            assertTrue(shows(RETITLED))
            assertFalse("announced again as $RETITLED", RETITLED in paneTitles())
        }
    }

    @Test @Config(qualifiers = SMALL_TABLET)
    fun anotherChatIsAnnouncedAsItOpens() = app {
        openChat()
        click("Release notes")
        repeat(3) { idle() }
        await { "Release notes" in paneTitles() }
    }

    // --- Fields named inside them ---

    @Test fun thePhoneComposerReadsItsTextWithItsName() = app {
        openChat()
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onLast()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(TYPED)) }
        idle()
        val field = compose.onAllNodes(hasSetTextAction()).fetchSemanticsNodes()
            .single { it.config.getOrNull(SemanticsProperties.EditableText)?.text == TYPED }
        // The typed text is the field's text; its name is read with it, and no description takes their place.
        assertTrue(field.config.getOrNull(SemanticsProperties.Text).orEmpty().any { it.text == COMPOSER_NAME })
        assertTrue(field.config.getOrNull(SemanticsProperties.ContentDescription).isNullOrEmpty())
        // Empty, the placeholder names it, once.
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onLast()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("")) }
        idle()
        val empty = compose.onAllNodes(hasSetTextAction()).fetchSemanticsNodes().last()
        assertEquals(listOf("Ask anything…"), empty.config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text })
    }

    // --- The file tree, walked once ---

    @Test fun theFileTreeIsWalkedOncePerTree() {
        val reads = AtomicInteger()
        val listing = mapOf("" to listOf(FileEntryReply("a.kt", directory = false), FileEntryReply("b.kt", directory = false)))
        val counted = object : Map<String, List<FileEntryReply>> by listing {
            override fun get(key: String): List<FileEntryReply>? {
                reads.incrementAndGet()
                return listing[key]
            }
        }
        val tree = FileTree(children = counted, expanded = setOf(""))
        var keystrokes by mutableIntStateOf(0)
        compose.setContent {
            val rows = rememberFileRows(tree)
            Column {
                Text("${rows.size} rows")
                Text("$keystrokes typed")
            }
        }
        compose.waitForIdle()
        val walked = reads.get()
        assertTrue(walked > 0)
        // Recomposed as a keystroke recomposes the pane: the same tree is not walked again.
        repeat(5) {
            compose.runOnUiThread { keystrokes++ }
            compose.waitForIdle()
        }
        assertEquals(walked, reads.get())
        val source = java.io.File("src/main/kotlin/com/restartfu/xd/mobile/ui/FilesPane.kt").readText()
        assertTrue("the Files pane walks the tree in composition", "rememberFileRows(state.tree)" in source)
        assertFalse(Regex("""state\.tree\.rows\(\)""").containsMatchIn(source))
    }

    // --- Undo ---

    @Test fun aDroppedQueuedMessageComesBackToItsPlace() = app(override = queued, afterReply = queueEvents) {
        motion(false)
        click("Reconnect backoff")
        await { shows(FIRST_QUEUED) }
        idle()
        click(FIRST_QUEUED)
        repeat(3) {
            ShadowLooper.idleMainLooper()
            idle()
        }
        click("Drop")
        await { sent("drop-queue") != null }
        await { shows(REMOVED_NOTICE) && shows("Undo") }
        click("Undo")
        await { sent("reorder-queue") != null }
        // Back at the end of the queue first, then one step to where it stood.
        assertEquals(FIRST_QUEUED, sent("queue")?.text("text"))
        val reorder = checkNotNull(sent("reorder-queue"))
        assertEquals(1, reorder.number("source"))
        assertEquals(0, reorder.number("anchor"))
        assertEquals(LATER_QUEUED, reorder.text("anchor-text"))
        assertEquals(false, reorder["after"]?.jsonPrimitive?.content?.toBoolean())
        await { !shows(REMOVED_NOTICE) }
    }

    @Test fun aRemovedPictureComesBackToItsPlace() {
        val first = png(0xff3366cc.toInt())
        val second = png(0xffcc6633.toInt())
        app(override = { op, request ->
            if (op == "chat" && request.text("chat") == "c1") {
                AuditHost.reply(op, request).replace(
                    """"queue":[]""",
                    """"queue":[],"draft":"look at these","draft_revision":1,"draft_attachments":[""" +
                        """{"name":"one.png","mime":"image/png","data":"$first"},{"name":"two.png","mime":"image/png","data":"$second"}]""",
                )
            } else {
                null
            }
        }) {
            motion(false)
            click("Reconnect backoff")
            await { compose.onAllNodes(hasContentDescription("Attached image"), useUnmergedTree = true).fetchSemanticsNodes().size == 2 }
            idle()
            compose.onAllNodes(hasContentDescription("Remove attached image"), useUnmergedTree = true)[0].performClick()
            idle()
            await { shows(REMOVED_NOTICE) && shows("Undo") }
            assertEquals(1, compose.onAllNodes(hasContentDescription("Attached image"), useUnmergedTree = true).fetchSemanticsNodes().size)
            click("Undo")
            await { compose.onAllNodes(hasContentDescription("Attached image"), useUnmergedTree = true).fetchSemanticsNodes().size == 2 }
            // The draft the host keeps has both again, in their order.
            // The draft goes to the host after a pause, on the main looper's clock.
            awaitLooper({ "drafts sent: ${checkNotNull(host).sent.filter { it.text("op") == "set-draft" }.map { r -> (r["attachments"] as? JsonArray)?.map { (it.jsonObject["data"] as JsonPrimitive).content.take(12) } }} want ${listOf(first, second).map { it.take(12) }}" }) {
                draftImages().lastOrNull() == listOf(first, second)
            }
        }
    }

    // --- Helpers ---

    private val withAgent: (String?, JsonObject) -> String? = { op, request ->
        if (op == "messages" && request.text("chat") == "c1") {
            val rows = AuditHost.MESSAGES.dropLast(1) + ("tool" to "subagent_run\n$AGENT") + AuditHost.MESSAGES.last()
            """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                rows.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
        } else {
            null
        }
    }

    private val queued: (String?, JsonObject) -> String? = { op, request ->
        if (op == "chat" && request.text("chat") == "c1") {
            AuditHost.reply(op, request)
                .replace(""""queue":[]""", """"queue":["$FIRST_QUEUED","$LATER_QUEUED"],"queue_revision":1""")
                .replace(""""working":false""", """"working":true""")
        } else {
            null
        }
    }

    private val queueEvents: (String?) -> String? = { op ->
        when (op) {
            "drop-queue" -> queueEvent(2, LATER_QUEUED)
            "queue" -> queueEvent(3, LATER_QUEUED, FIRST_QUEUED)
            "reorder-queue" -> queueEvent(4, FIRST_QUEUED, LATER_QUEUED)
            else -> null
        }
    }

    private fun queueEvent(revision: Int, vararg queue: String) =
        """{"event":"queued","chat":"c1","queue":[${queue.joinToString(",") { "\"$it\"" }}],"queue_revision":$revision}"""

    private fun withoutSession(tree: String, id: String): String {
        val json = kotlinx.serialization.json.Json.parseToJsonElement(tree).jsonObject
        val sessions = (json["sessions"] as JsonArray).filterNot { (it.jsonObject["id"] as JsonPrimitive).content == id }
        val chats = (json["chats"] as JsonArray).filterNot { (it.jsonObject["session"] as? JsonPrimitive)?.content == id }
        return JsonObject(json + ("sessions" to JsonArray(sessions)) + ("chats" to JsonArray(chats))).toString()
    }

    private fun draftImages(): List<List<String>> = checkNotNull(host).sent
        .filter { it.text("op") == "set-draft" && it["attachments"] != null }
        .map { request -> (request["attachments"] as JsonArray).map { (it.jsonObject["data"] as JsonPrimitive).content } }

    private fun png(colour: Int): String {
        val bitmap = Bitmap.createBitmap(4, 4, Bitmap.Config.ARGB_8888)
        bitmap.eraseColor(colour)
        val bytes = ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            out.toByteArray()
        }
        return Base64.encodeToString(bytes, Base64.NO_WRAP)
    }

    private fun motion(on: Boolean) {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, if (on) 1f else 0f)
        idle()
    }

    private fun openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        repeat(2) { idle() }
    }

    private fun openAgent() {
        openChat()
        await { shows("Open") }
        click("Open")
        repeat(2) { idle() }
        await { shows(AGENT_PROMPT) && !shows(CHAT_MARK) }
    }

    private fun openModel() {
        click("Usage")
        click("Cost")
        await { sent("usage-summary") != null }
        checkNotNull(host).push(
            """{"event":"usage-summary","generation":${sent("usage-summary")?.text("generation")},"summary":$SUMMARY}""",
        )
        await { shows("gpt-6-luna") }
        idle()
        click("gpt-6-luna")
        repeat(2) { idle() }
        await { shows("Back to overview") && !shows("Sessions") }
    }

    private fun opened(): Int = checkNotNull(host).sent.count { it.text("op") in setOf("terminal-list", "terminal-open") }

    private fun sent(op: String): JsonObject? = checkNotNull(host).sent.lastOrNull { it.text("op") == op }

    private fun railItem(label: String) = hasText(label) and SemanticsMatcher.keyIsDefined(SemanticsProperties.Selected)

    /** Whether the merged tree, where a rail item's label and its selection are one node, has [matcher]. */
    private fun railShows(matcher: SemanticsMatcher): Boolean = compose.onAllNodes(matcher).fetchSemanticsNodes().isNotEmpty()

    private fun focusedText(): String {
        val node = compose.onAllNodes(isFocused()).fetchSemanticsNodes().single()
        return node.config.getOrNull(SemanticsProperties.Text).orEmpty().joinToString(" ") { it.text }
    }

    private fun paneTitles(): List<String> = compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.PaneTitle), useUnmergedTree = true)
        .fetchSemanticsNodes().map { it.config[SemanticsProperties.PaneTitle] }

    private fun edge(progress: Float) = BackEventCompat(0f, 400f, progress, BackEventCompat.EDGE_LEFT)

    private var content: (@Composable () -> Unit)? = null

    private fun launch(content: @Composable () -> Unit) {
        this.content = content
        compose.activityRule.scenario.onActivity { activity -> activity.setContent { content() } }
    }

    /** Destroys the activity and makes it again, as a rotation does, keeping what Android keeps. */
    private fun recreate() {
        compose.activityRule.scenario.recreate()
        launch(checkNotNull(content))
        idle()
    }

    private fun app(
        override: (String?, JsonObject) -> String? = { _, _ -> null },
        afterReply: (String?) -> String? = { null },
        theme: MinimalThemePreset = MinimalThemePreset.DARK,
        steps: () -> Unit,
    ) {
        val activity = compose.activity
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        val scripted = AuditHost(override = override, afterReply = afterReply).connect().also { host = it }
        model = MainViewModel(activity.application, scripted.client)
        val settings = MobileSettings(activity)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        launch {
            XdRoot(theme) {
                val live by model.client.link.collectAsStateWithLifecycle()
                MinimalMobileApp(model, live, settings)
            }
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        idle()
        steps()
    }

    private fun shows(label: String, substring: Boolean = false): Boolean = exists(
        hasText(label, substring = substring) or hasContentDescription(label, substring = substring),
    )

    private fun exists(matcher: SemanticsMatcher): Boolean =
        compose.onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun click(label: String) {
        compose.onAllNodes(hasText(label) or hasContentDescription(label), useUnmergedTree = true).onLast().performClick()
        idle()
    }

    private fun pressBack() {
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
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

    /** As [await], with the main looper's clock run on too, for work that waits on it. */
    private fun awaitLooper(why: () -> String = { "" }, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(100))
            settle()
        }
        check(condition()) { "Timed out ${why()}" }
    }

    private fun await(why: () -> String = { "" }, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(condition()) { "Timed out ${why()}" }
    }

    private companion object {
        const val SMALL_TABLET = "w840dp-h600dp-320dpi"
        const val LANDSCAPE_PHONE = "w780dp-h360dp-420dpi"
        // A line of the open chat's transcript, which its first screen always shows.
        const val CHAT_MARK = "3 tool calls"
        const val RETITLED = "Cap the reconnect backoff"
        const val TYPED = "Look at the backoff"
        const val FIRST_QUEUED = "Also bump the version"
        const val LATER_QUEUED = "Then update the notes"
        const val AGENT_PROMPT = "Audit how pairing retries after a dropped connection."
        const val AGENT = """{"key":"toolu_audit","task_id":"a22197aff550fdfd9","provider":"Claude","agent":"general-purpose",""" +
            """"description":"Audit pairing retries","status":"completed","model":"claude-opus-5-5",""" +
            """"prompt":"$AGENT_PROMPT","steps":[{"tool":"Read","text":"PairingInvitation.kt"}],"tool_uses":1,""" +
            """"result":"Pairing retries are capped."}"""

        val SUMMARY: String = run {
            val days = (1..3).map { "2026-11-0$it" }
            val periods = days.mapIndexed { i, day -> """{"key":"$day","label":"Nov ${i + 1}"}""" }
            val buckets = days.mapIndexed { i, day ->
                """{"day":"$day","provider":"codex","model":"gpt-6-luna","cost_usd":${(i + 1) * 0.4},""" +
                    """"totals":{"uncached_input":${(i + 1) * 10000},"output":${(i + 1) * 2000}}},""" +
                    """{"day":"$day","provider":"claude","model":"claude-opus-5-5","cost_usd":${(i + 1) * 0.3},""" +
                    """"totals":{"uncached_input":${(i + 1) * 6000},"output":${(i + 1) * 1500}}}"""
            }
            """{"window_label":"Past 7 days","periods":[${periods.joinToString(",")}],""" +
                """"buckets":[${buckets.joinToString(",")}],"sessions":{"codex":5,"claude":3}}"""
        }

        fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content
        fun JsonObject.number(name: String): Int? = (this[name] as? JsonPrimitive)?.content?.toIntOrNull()
    }
}
