package com.restartfu.xd.mobile.ui

import android.app.Dialog
import android.os.Bundle
import android.os.Parcel
import android.view.View
import android.view.ViewGroup
import androidx.activity.BackEventCompat
import androidx.activity.ComponentActivity
import androidx.activity.ComponentDialog
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.files.FileTab
import com.restartfu.xd.files.OpenFile
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.UNSAVED_LIMIT_BYTES
import com.restartfu.xd.mobile.UnsavedFileStore
import com.restartfu.xd.model.ChatSummary
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowDialog
import org.robolectric.shadows.ShadowLooper

/**
 * The fifth audit's fixes: the snackbar above the keyboard and the composer,
 * the settling stepper as one adjustable control that says its value,
 * errors told as they appear, unsaved file edits that outlive Android ending
 * the app, a predictive Back between New session's steps, and the window's
 * layout from the room the system bars leave.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxFifthTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null
    private lateinit var model: MainViewModel

    @After fun close() {
        host?.close()
    }

    // --- Where the snackbar sits ---

    @Test fun aNoticeStaysAboveTheKeyboard() = app {
        showKeyboard(compose.activity, px(KEYBOARD).toInt(), px(NAVIGATION).toInt())
        idle()
        notice()
        val keyboardTop = root().bottom - px(KEYBOARD) - px(NAVIGATION)
        val snackbar = snackbar()
        assertTrue("the snackbar ends at ${snackbar.bottom}, the keyboard starts at $keyboardTop", snackbar.bottom <= keyboardTop)
    }

    @Test fun aNoticeInAChatStaysAboveTheComposer() = app {
        openChat()
        notice()
        val field = composerField()
        val snackbar = snackbar()
        assertTrue("the snackbar ends at ${snackbar.bottom}, the composer starts at ${field.top}", snackbar.bottom <= field.top)
    }

    @Test fun aNoticeInAChatStaysAboveTheComposerWithTheKeyboardUp() = app {
        openChat()
        showKeyboard(compose.activity, px(KEYBOARD).toInt(), px(NAVIGATION).toInt())
        idle()
        notice()
        val keyboardTop = root().bottom - px(KEYBOARD) - px(NAVIGATION)
        val field = composerField()
        assertTrue("the composer ends at ${field.bottom}, under the keyboard at $keyboardTop", field.bottom <= keyboardTop + 1f)
        val snackbar = snackbar()
        assertTrue("the snackbar ends at ${snackbar.bottom}, the composer starts at ${field.top}", snackbar.bottom <= field.top)
    }

    @Test fun snackbarPlacementKeepsClearOfWhatIsBelow() {
        // A composer already stands above the keyboard, so only the gap is added to it.
        assertEquals(208.dp, snackbarBottom(composer = 200.dp, insets = 348.dp, above = 24.dp))
        assertEquals(372.dp, snackbarBottom(composer = 0.dp, insets = 348.dp, above = 24.dp))
    }

    // --- The settling stepper ---

    @Test fun theStepperIsOneAdjustableStopThatSaysItsValue() = app {
        cleanup()
        val row = compose.onNode(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "3 days")).fetchSemanticsNode()
        assertTrue(row.texts().joinToString().startsWith("Days before settling"))
        // Told whenever the number changes, from these buttons or another device.
        assertEquals(LiveRegionMode.Polite, row.config.getOrNull(SemanticsProperties.LiveRegion))
        val range = row.config[SemanticsProperties.ProgressBarRangeInfo]
        assertEquals(3f, range.current)
        assertEquals(1f..90f, range.range)
        // The digits are not read twice.
        assertFalse(row.texts().contains("3"))
        for (name in listOf("Fewer days", "More days")) {
            val button = compose.onNode(hasContentDescription(name)).fetchSemanticsNode()
            assertEquals(Role.Button, button.config.getOrNull(SemanticsProperties.Role))
            assertFalse(button.config.contains(SemanticsProperties.Disabled))
            assertTrue("$name is ${button.size}", button.size.width >= px(48.dp) - 1 && button.size.height >= px(48.dp) - 1)
        }
        // Swiping up or down on the row sets it as a slider does.
        compose.onNode(hasSetProgress()).performSemanticsAction(SemanticsActions.SetProgress) { it(10f) }
        idle()
        await { sentDays().contains(10) }
    }

    @Test fun moreDaysAsksForOneMore() = app {
        cleanup()
        compose.onNode(hasContentDescription("More days")).performClick()
        idle()
        await { sentDays().contains(4) }
    }

    @Test fun atItsEndTheStepperSaysWhyItStops() = app(override = { op, _ ->
        if (op == "auto-settle-settings") """{"ok":true,"after_days":1,"on_merge":true,"keep_pull_requests":false}""" else null
    }) {
        cleanup("1 day")
        val fewer = compose.onNode(hasContentDescription("Fewer days")).fetchSemanticsNode()
        assertTrue(fewer.config.contains(SemanticsProperties.Disabled))
        val row = compose.onNode(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, "1 day")).fetchSemanticsNode()
        assertTrue(row.texts().joinToString(), row.texts().any { "1 day is the shortest." in it })
        assertFalse(compose.onNode(hasContentDescription("More days")).fetchSemanticsNode().config.contains(SemanticsProperties.Disabled))
    }

    // --- Errors told as they appear ---

    @Test fun aRuleThatCouldNotBeSavedIsTold() = app(override = { op, _ ->
        if (op == "set-auto-settle-settings") """{"ok":false,"error":"The host could not save the rules."}""" else null
    }) {
        cleanup()
        compose.onNode(hasContentDescription("More days")).performClick()
        idle()
        await { shows("The host could not save the rules.") }
        assertAnnounced("The host could not save the rules.")
    }

    @Test fun aFileThatCouldNotBeSavedIsTold() {
        val scripted = AuditHost(override = { op, request ->
            if (op == "file-browse" && request.text("action") == "write") """{"ok":false,"error":"The file changed on disk."}""" else null
        }).connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val files = FilesViewModel { session }
        content { FilesPaneContent(files) }
        await { shows("README.md") }
        click("README.md")
        await { files.state.value.open.current != null }
        idle()
        type("# xd\n\nNot saved.\n")
        click("Save")
        await { shows("The file changed on disk.") }
        assertAnnounced("The file changed on disk.")
    }

    @Test fun theQueueNoticeAndAnAttachmentErrorAreTold() = chat { chat ->
        compose.runOnUiThread { chat.queueChanged() }
        idle()
        await { shows("The queue changed on another device. Open the message again.") }
        assertAnnounced("The queue changed on another device. Open the message again.")
        // A picture that cannot be read is refused, and the reason is told.
        compose.runOnUiThread { chat.attach(compose.activity, listOf(android.net.Uri.parse("content://nowhere/1"))) }
        await { chat.attachmentError.value != null }
        idle()
        assertAnnounced(checkNotNull(chat.attachmentError.value))
    }

    // --- Unsaved edits outlive the end of the process ---

    @Test fun anUnsavedEditComesBackAfterAndroidEndsTheApp() {
        val scripted = AuditHost().connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val first = Robolectric.buildActivity(ComponentActivity::class.java).setup()
        val before = ViewModelProvider(first.get())[MobileWorkspaceModels::class.java]
        val files = FilesViewModel(before.unsavedFiles("s1")) { session }
        files.open("README.md")
        await { files.state.value.open.current != null }
        val original = files.state.value.open.current!!.saved
        files.edit("README.md", "# xd\n\nKept through the end.\n")
        ShadowLooper.idleMainLooper()
        // Android saves the activity, ends the process, and starts it again
        // from what it saved: a new activity, new view models, the same bundle.
        val saved = Bundle()
        first.saveInstanceState(saved).pause().stop().destroy()
        val restored = parcelled(saved)
        val second = Robolectric.buildActivity(ComponentActivity::class.java).setup(restored)
        val after = ViewModelProvider(second.get())[MobileWorkspaceModels::class.java]
        assertTrue("the view models were made again", after !== before)
        val back = FilesViewModel(after.unsavedFiles("s1")) { session }
        val file = back.state.value.open.files.single()
        assertEquals("README.md", file.path)
        assertEquals("# xd\n\nKept through the end.\n", file.text)
        // Still unsaved, against what the host last handed over, and in front.
        assertTrue(file.dirty)
        assertEquals(original, file.saved)
        assertEquals(FileTab.File("README.md"), back.state.value.open.active)
        // The discard question still guards its tab.
        content { FilesPaneContent(back) }
        click("Close README.md")
        await { shows("Discard changes to README.md?") }
        second.pause().stop().destroy()
    }

    @Test fun editsPastTheLimitAreNotKeptAndThePaneSaysSo() {
        val store = UnsavedFileStore(SavedStateHandle(), limitBytes = 200)
        val small = OpenFile("a.txt", saved = "a", text = "ab")
        val large = OpenFile("big.txt", saved = "x".repeat(60), text = "y".repeat(60))
        assertEquals(setOf("big.txt"), store.session("s1").keep(listOf(small, large)))
        assertEquals(listOf(small), store.session("s1").restore())
        // The limit is shared by every session.
        assertEquals(setOf("c.txt"), store.session("s2").keep(listOf(OpenFile("c.txt", saved = "", text = "z".repeat(95)))))
        assertEquals(512L * 1024, UNSAVED_LIMIT_BYTES)

        val scripted = AuditHost(override = { op, request ->
            if (op == "file-browse" && request.text("action") == "read") """{"ok":true,"content":"short"}""" else null
        }).connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val tiny = UnsavedFileStore(SavedStateHandle(), limitBytes = 64)
        val files = FilesViewModel(tiny.session("s1")) { session }
        content { FilesPaneContent(files) }
        await { shows("README.md") }
        click("README.md")
        await { files.state.value.open.current != null }
        idle()
        type("x".repeat(40))
        await { shows(UNKEPT_EDIT_NOTICE) }
        assertAnnounced(UNKEPT_EDIT_NOTICE)
    }

    // --- New session's steps ---

    @Test fun backFromTheFormShowsTheListAsItIsDragged() = app {
        // New, then New session: the list of projects first, as there are two.
        click("New")
        compose.onAllNodes(hasText("New session"), useUnmergedTree = true).onLast().performClick()
        idle()
        awaitOr({ "the list shows ${dialogTexts()}" }) { dialogTexts().contains("web") }
        compose.onAllNodes(hasText("xd") and hasAnyAncestor(isDialog()), useUnmergedTree = true).onLast().performClick()
        repeat(4) { idle() }
        // The form for xd: the list, and web in it, has gone.
        assertFalse(dialogTexts().joinToString(), dialogTexts().contains("web"))
        val back = (ShadowDialog.getLatestDialog() as ComponentDialog).onBackPressedDispatcher
        compose.runOnUiThread {
            back.dispatchOnBackStarted(BackEventCompat(0f, 900f, 0f, BackEventCompat.EDGE_LEFT))
            back.dispatchOnBackProgressed(BackEventCompat(200f, 900f, 0.5f, BackEventCompat.EDGE_LEFT))
        }
        repeat(2) { idle() }
        assertTrue("the list comes in under the gesture: ${dialogTexts()}", dialogTexts().contains("web"))
        // Called off, the form stays.
        compose.runOnUiThread { back.dispatchOnBackCancelled() }
        repeat(3) { idle() }
        assertFalse(dialogTexts().contains("web"))
        // Done, Back returns to the list.
        compose.runOnUiThread { back.onBackPressed() }
        repeat(3) { idle() }
        assertTrue(dialogTexts().contains("web"))
    }

    // --- The window's size ---

    @Test @Config(qualifiers = "w900dp-h500dp-160dpi")
    fun theSystemBarsAreNotRoomForTheTabletHeader() {
        var layout: WindowLayout? = null
        compose.setContent { layout = rememberWindowLayout() }
        compose.waitForIdle()
        assertTrue("without bars 500dp is tall enough", layout!!.tabletChat)
        // A status bar and a navigation bar take 72dp of the 500 the system reports.
        val insets = WindowInsetsCompat.Builder()
            .setInsets(WindowInsetsCompat.Type.statusBars(), Insets.of(0, px(24.dp).toInt(), 0, 0))
            .setInsets(WindowInsetsCompat.Type.navigationBars(), Insets.of(0, 0, 0, px(48.dp).toInt()))
            .setVisible(WindowInsetsCompat.Type.statusBars(), true)
            .setVisible(WindowInsetsCompat.Type.navigationBars(), true)
            .build()
        compose.runOnUiThread { ViewCompat.dispatchApplyWindowInsets(composeViewOf(compose.activity), insets) }
        compose.waitForIdle()
        assertEquals(428, layout!!.heightDp)
        assertEquals(900, layout!!.widthDp)
        assertTrue("the rail still fits", layout!!.rail)
        assertFalse("428dp is a compact height", layout!!.tabletChat)
    }

    // --- Helpers ---

    private fun app(
        override: (String?, JsonObject) -> String? = { _, _ -> null },
        steps: () -> Unit,
    ) {
        val activity = compose.activity
        val scripted = AuditHost(override = override).connect().also { host = it }
        model = MainViewModel(activity.application, scripted.client)
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

    private fun chat(steps: (ChatViewModel) -> Unit) {
        val scripted = AuditHost().connect().also { host = it }
        val chat = ChatViewModel(scripted.client, "c1")
        val chats = listOf(ChatSummary("c1", "xd", "Chat", "claude", false, sessionId = "s1"))
        content {
            ChatScreen(
                model = chat,
                goBack = {},
                sessionTitle = "Reconnect backoff",
                projectName = "xd",
                sessionChats = SessionChats(chats, "c1", {}, {}, {}, {}, {}),
            )
        }
        await { shows("3 tool calls") }
        idle()
        steps(chat)
    }

    private fun content(content: @Composable () -> Unit) {
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(MinimalThemePreset.DARK) { Box { content() } } }
        settle()
    }

    private fun openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        idle()
    }

    /** The notice an unreadable pairing code gives, on the app's snackbar. */
    private fun notice() {
        compose.runOnUiThread { model.connectPairing("", "") }
        idle()
        await { exists(hasText("Tailscale address", substring = true)) }
    }

    private fun cleanup(days: String = "3 days") {
        click("Settings")
        await { shows("Search settings") }
        compose.onAllNodes(hasText("Cleanup")).onFirst().performClick()
        idle()
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.StateDescription, days)) }
    }

    private fun sentDays(): List<Int> = checkNotNull(host).sent
        .filter { (it["op"] as? JsonPrimitive)?.content == "set-auto-settle-settings" }
        .mapNotNull { (it["after_days"] as? JsonPrimitive)?.content?.toIntOrNull() }

    private fun hasSetProgress() = SemanticsMatcher.keyIsDefined(SemanticsActions.SetProgress)

    /** The snackbar's container, which Material gives the Dismiss action. */
    private fun snackbar(): Rect = compose.onNode(SemanticsMatcher.keyIsDefined(SemanticsActions.Dismiss)).fetchSemanticsNode().boundsInRoot

    private fun composerField(): Rect =
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onLast().fetchSemanticsNode().boundsInRoot

    private fun root(): Rect = compose.onNode(isRoot()).fetchSemanticsNode().boundsInRoot

    private fun assertAnnounced(text: String) {
        val nodes = compose.onAllNodes(hasText(text), useUnmergedTree = true).fetchSemanticsNodes()
        assertTrue("no $text", nodes.isNotEmpty())
        assertTrue("$text is not told as it appears", nodes.any { it.config.getOrNull(SemanticsProperties.LiveRegion) == LiveRegionMode.Polite })
    }

    /** What the latest dialog shows, read from its own window without waiting for it to go idle. */
    private fun dialogTexts(): List<String> {
        val dialog: Dialog = ShadowDialog.getLatestDialog() ?: return emptyList()
        fun find(view: View): View? = when (view) {
            is ViewRootForTest -> view
            is ViewGroup -> (0 until view.childCount).firstNotNullOfOrNull { find(view.getChildAt(it)) }
            else -> null
        }
        val root = find(dialog.window!!.decorView) as? RootForTest ?: return emptyList()
        fun walk(node: SemanticsNode): List<SemanticsNode> = listOf(node) + node.children.flatMap(::walk)
        return walk(root.semanticsOwner.unmergedRootSemanticsNode).flatMap { it.texts() }
    }

    private fun type(text: String) {
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onFirst()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(text)) }
        idle()
    }

    private fun parcelled(bundle: Bundle): Bundle {
        val parcel = Parcel.obtain()
        try {
            parcel.writeBundle(bundle)
            parcel.setDataPosition(0)
            return checkNotNull(parcel.readBundle(javaClass.classLoader))
        } finally {
            parcel.recycle()
        }
    }

    private fun shows(label: String): Boolean = exists(hasText(label) or hasContentDescription(label))

    private fun exists(matcher: SemanticsMatcher): Boolean =
        compose.onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun click(label: String) {
        compose.onAllNodes(hasText(label) or hasContentDescription(label), useUnmergedTree = true).onLast().performClick()
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

    private fun awaitOr(message: () -> String, condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(condition(), message)
    }

    private fun px(size: Dp): Float = with(compose.density) { size.toPx() }

    private fun SemanticsNode.texts(): List<String> =
        config.getOrNull(SemanticsProperties.Text).orEmpty().map { it.text } +
            config.getOrNull(SemanticsProperties.ContentDescription).orEmpty()

    private companion object {
        val KEYBOARD = 300.dp
        val NAVIGATION = 48.dp

        fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content
    }
}
