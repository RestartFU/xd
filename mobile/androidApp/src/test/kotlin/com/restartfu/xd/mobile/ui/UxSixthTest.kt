package com.restartfu.xd.mobile.ui

import android.app.Dialog
import android.os.Bundle
import android.os.Looper
import android.os.Parcel
import android.provider.Settings
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.node.RootForTest
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MAX_PENDING_ERRORS
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.UNSENT_DRAFT_LIMIT_BYTES
import com.restartfu.xd.mobile.UnsentDrafts
import com.restartfu.xd.mobile.XdMobileApp
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.SocketFailureKind
import java.time.Duration
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowDialog
import org.robolectric.shadows.ShadowLooper

/**
 * The sixth audit's fixes: an operation's failure said once, never again
 * after a rotation nor on the fatal screen; composer drafts that outlive
 * going offline, the background and the end of the process; the floating
 * bar's selected tab at 3:1; the tablet header's model button saying what it
 * does; New session as a basic dialog on tablets; one watch on "Remove
 * animations"; and the file editor's keystrokes costing what they type.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxSixthTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null
    private lateinit var model: MainViewModel

    @After fun close() {
        host?.close()
    }

    // --- An operation's failure, said once ---

    @Test fun aFailureIsSaidOnceAndNotAgainAfterARotation() = app(override = refusingDelete) {
        compose.runOnUiThread { model.deleteChat("c9") }
        await { shows(REFUSAL) }
        // Its snackbar times out, and the failure is spent.
        await { !shows(REFUSAL) }
        assertTrue(model.operationErrors.value.isEmpty())
        recreate()
        repeat(6) { idle() }
        assertFalse("said again after the rotation", shows(REFUSAL))
    }

    @Test fun aFailureShowingDuringARotationIsShownToTheEnd() = app(override = refusingDelete) {
        compose.runOnUiThread { model.deleteChat("c9") }
        await { shows(REFUSAL) }
        recreate()
        // Not yet gone when the window turned, so it shows in the new one, once.
        await { shows(REFUSAL) }
        await { !shows(REFUSAL) }
        recreate()
        repeat(6) { idle() }
        assertFalse(shows(REFUSAL))
    }

    @Test fun aFailureWithNoSnackbarOnScreenWaitsForOne() {
        val scripted = AuditHost().connect().also { host = it }
        model = MainViewModel(compose.activity.application, scripted.client)
        val settings = MobileSettings(compose.activity)
        var shown by mutableStateOf(false)
        compose.mainClock.autoAdvance = false
        launch {
            val link by model.client.link.collectAsStateWithLifecycle()
            if (shown) MinimalMobileApp(model, link, settings)
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        compose.runOnUiThread { model.reportError(REFUSAL) }
        repeat(3) { idle() }
        assertEquals(listOf(REFUSAL), model.operationErrors.value.map { it.message })
        compose.runOnUiThread { shown = true }
        await { shows(REFUSAL) }
        await { model.operationErrors.value.isEmpty() }
    }

    @Test fun onlyTheNewestFailuresWait() {
        val scripted = AuditHost().connect().also { host = it }
        model = MainViewModel(compose.activity.application, scripted.client)
        compose.runOnUiThread { repeat(5) { model.reportError("failure $it") } }
        assertEquals((5 - MAX_PENDING_ERRORS until 5).map { "failure $it" }, model.operationErrors.value.map { it.message })
    }

    @Test fun theFatalScreenSaysNothingOfAnEarlierFailure() = app(override = refusingDelete, whole = true) {
        compose.runOnUiThread { model.deleteChat("c9") }
        await { shows(REFUSAL) }
        checkNotNull(host).drop(SocketFailureKind.AUTHENTICATION, "This device was removed on the host.")
        await { shows("Host connection failed") }
        repeat(3) { idle() }
        assertTrue(shows("This device was removed on the host."))
        assertFalse("an old failure under the fatal screen", shows(REFUSAL))
        assertTrue(model.operationErrors.value.isEmpty())
    }

    // --- Drafts the host has not taken ---

    @Test fun aDraftTypedOfflineComesBackAfterAndroidEndsTheAppAndGoesWhenOnline() {
        val scripted = AuditHost().connect().also { host = it }
        goOffline(scripted)
        val first = Robolectric.buildActivity(ComponentActivity::class.java).setup()
        val before = ViewModelProvider(first.get(), UnsentDrafts.factory(scripted.client))[UnsentDrafts::class.java]
        val chat = ChatViewModel.Factory(scripted.client, "c1", before).create(ChatViewModel::class.java)
        chat.updateDraft(DRAFT)
        // Its push, a pause after typing, fails offline; the draft stays kept.
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(400))
        assertEquals(DRAFT, before.kept("c1"))
        // Android saves the activity, ends the process, and starts it again.
        val saved = Bundle()
        first.saveInstanceState(saved).pause().stop().destroy()
        val second = Robolectric.buildActivity(ComponentActivity::class.java).setup(parcelled(saved))
        val after = ViewModelProvider(second.get(), UnsentDrafts.factory(scripted.client))[UnsentDrafts::class.java]
        assertTrue("the view models were made again", after !== before)
        assertEquals(DRAFT, after.kept("c1"))
        val back = ChatViewModel.Factory(scripted.client, "c1", after).create(ChatViewModel::class.java)
        assertEquals(DRAFT, back.draft.value)
        assertEquals(0, draftsSent(scripted, "c1", DRAFT))
        // Back online: the draft goes to the host, and the host's empty one does not replace it.
        scripted.offline = false
        scripted.client.retryConnection()
        awaitLooper { draftsSent(scripted, "c1", DRAFT) >= 1 }
        awaitLooper { after.kept("c1") == null }
        awaitLooper { back.state.value.draftLoaded }
        assertEquals(DRAFT, back.draft.value)
        second.pause().stop().destroy()
    }

    @Test fun aKeptDraftWhoseChatIsNotOpenGoesWhenOnline() {
        val scripted = AuditHost().connect().also { host = it }
        goOffline(scripted)
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup()
        val drafts = ViewModelProvider(activity.get(), UnsentDrafts.factory(scripted.client))[UnsentDrafts::class.java]
        drafts.keep("c1b", DRAFT)
        scripted.offline = false
        scripted.client.retryConnection()
        awaitLooper { draftsSent(scripted, "c1b", DRAFT) == 1 }
        awaitLooper { drafts.kept("c1b") == null }
        activity.pause().stop().destroy()
    }

    @Test fun draftsPastTheLimitAreNotKept() {
        val scripted = AuditHost().also { host = it }
        val drafts = UnsentDrafts(androidx.lifecycle.SavedStateHandle(), scripted.client, limitBytes = 100)
        assertTrue(drafts.keep("a", "x".repeat(20)))
        // 2 bytes a character, the chat id included: 2 * (1 + 30) = 62 does not fit beside 42.
        assertFalse(drafts.keep("b", "y".repeat(30)))
        assertNull(drafts.kept("b"))
        // Grown past the limit, a draft is no longer kept at all, not kept stale.
        assertFalse(drafts.keep("a", "x".repeat(60)))
        assertNull(drafts.kept("a"))
        // Sent, it goes; edited since, it stays.
        drafts.keep("c", "one")
        drafts.sent("c", "on")
        assertEquals("one", drafts.kept("c"))
        drafts.sent("c", "one")
        assertNull(drafts.kept("c"))
        // Forgetting the host lets them go; the first binding, a restart, does not.
        drafts.keep("d", "kept")
        drafts.bindGeneration(0)
        assertEquals("kept", drafts.kept("d"))
        drafts.bindGeneration(1)
        assertNull(drafts.kept("d"))
        assertEquals(128L * 1024, UNSENT_DRAFT_LIMIT_BYTES)
    }

    @Test fun goingToTheBackgroundSendsTheDraftAtOnce() = app(whole = true) {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        idle()
        composer().performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(DRAFT)) }
        idle()
        // Within the pause after typing: nothing sent yet.
        assertEquals(0, draftsSent(checkNotNull(host), "c1", DRAFT))
        compose.activityRule.scenario.moveToState(Lifecycle.State.CREATED)
        await { draftsSent(checkNotNull(host), "c1", DRAFT) == 1 }
        // Then the connection closes.
        await { model.client.link.value is Link.Idle }
    }

    @Test fun aDraftTypedOfflineInTheAppGoesWhenTheConnectionComesBack() = app {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        idle()
        val scripted = checkNotNull(host)
        goOffline(scripted)
        composer().performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(DRAFT)) }
        // Past the pause after typing: its push fails, offline.
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(400))
        idle()
        assertEquals(0, draftsSent(scripted, "c1", DRAFT))
        scripted.offline = false
        compose.runOnUiThread { scripted.client.retryConnection() }
        await { draftsSent(scripted, "c1", DRAFT) >= 1 }
    }

    // --- The floating bar's selected tab ---

    @Test fun theSelectedTabIsEdgedAtThreeToOneOnTheBar() {
        for (theme in MinimalThemePreset.entries) {
            val ratio = contrastRatio(xdSignals(theme).controlBorder, xdGlass(theme).overlay())
            assertTrue("$theme: the selected tab's edge is $ratio:1 on the bar", ratio >= 3f)
        }
    }

    @Test fun theSelectedTabStandsOutOnTheBarDark() = selectedTabContrast(MinimalThemePreset.DARK)

    @Test fun theSelectedTabStandsOutOnTheBarLight() = selectedTabContrast(MinimalThemePreset.LIGHT)

    private fun selectedTabContrast(theme: MinimalThemePreset) = app(theme = theme) {
        val tab = compose.onNode(hasContentDescription("Projects") and SemanticsMatcher.expectValue(SemanticsProperties.Selected, true))
            .fetchSemanticsNode().boundsInWindow
        val bitmap = ProjectsScreenshotTest.Screen(compose, compose.activity).capture()
        val y = tab.center.y.toInt()
        val edge = Color(bitmap.getPixel(tab.left.toInt() + 1, y))
        val bar = Color(bitmap.getPixel(tab.left.toInt() - 3, y))
        val ratio = contrastRatio(edge, bar)
        assertTrue("$theme: the selected tab is $ratio:1 on the bar", ratio >= 3f)
    }

    // --- The tablet header's model button ---

    @Test @Config(qualifiers = TABLET)
    fun theTabletModelButtonSaysWhatItDoes() = app {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        idle()
        val buttons = compose.onAllNodes(hasText("Opus 5.5") and hasClickAction()).fetchSemanticsNodes()
        assertTrue("no model button", buttons.isNotEmpty())
        for (button in buttons) {
            assertEquals("Choose model", button.config[SemanticsActions.OnClick].label)
        }
    }

    // --- New session's shape ---

    @Test @Config(qualifiers = TABLET)
    fun newSessionIsADialogOnATablet() = app {
        click("New session in xd")
        repeat(4) { idle() }
        val (root, close) = dialogBounds("Close")
        val side = (root.width - px(560.dp)) / 2
        assertTrue("the dialog starts at ${close.left} in a window ${root.width} wide", close.left >= side - 1)
        val (_, heading) = dialogBounds("New session")
        assertTrue(heading.right <= root.width - side + 1)
        // Its own top and bottom bars, inside the window's margins.
        assertTrue("close at ${close.top}", close.top >= px(24.dp))
    }

    @Test @Config(qualifiers = "w360dp-h780dp-420dpi")
    fun newSessionIsFullScreenOnAPhone() = app {
        click("New session in xd")
        repeat(4) { idle() }
        val (_, close) = dialogBounds("Close")
        assertTrue("close at ${close.left}", close.left < px(24.dp))
    }

    // --- "Remove animations", watched once ---

    @Test fun oneWatchServesEveryMarkThatAsks() {
        val resolver = compose.activity.contentResolver
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
        val uri = Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE)
        val reads = BooleanArray(40)
        compose.setContent {
            XdRoot(MinimalThemePreset.DARK) {
                Column { for (index in reads.indices) reads[index] = reducedMotion() }
            }
        }
        compose.waitForIdle()
        assertEquals(1, shadowOf(resolver).getContentObservers(uri).size)
        assertTrue(reads.none { it })
        // Still live: turning animations off reaches every reader.
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        resolver.notifyChange(uri, null)
        ShadowLooper.idleMainLooper()
        compose.waitForIdle()
        assertTrue(reads.all { it })
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
    }

    // --- The file editor's keystrokes ---

    @Test fun lineBreaksAreCountedFromTheEdit() {
        fun value(text: String, start: Int, end: Int = start) = TextFieldValue(text, TextRange(start, end))
        val text = "one\ntwo\nthree"
        // Typed at the caret.
        assertEquals(3, newlinesAfterEdit(value(text, 3), value("one\n\ntwo\nthree", 4), 2))
        // Pasted over a selection holding a break.
        assertEquals(3, newlinesAfterEdit(value(text, 2, 5), value("on\n\nwo\nthree", 4), 2))
        // Backspace over a break.
        assertEquals(1, newlinesAfterEdit(value(text, 4), value("onetwo\nthree", 3), 2))
        // Delete of the break after the caret.
        assertEquals(1, newlinesAfterEdit(value(text, 3), value("onetwo\nthree", 3), 2))
        // A selection deleted.
        assertEquals(0, newlinesAfterEdit(value(text, 2, 9), value("onhree", 2), 2))
        // Anything else is counted afresh, so it is never wrong.
        assertEquals(1, newlinesAfterEdit(value(text, 0), value("x\ny", 2), 2))
    }

    @Test fun theEditorNamesEachTextByARevisionNotItsHash() {
        val editor = EditorText("fun a() {}\n")
        val first = editor.revision
        val typed = editor.edit(TextFieldValue("fun a() {}\n\n", TextRange(12)))
        assertEquals("fun a() {}\n\n", typed)
        assertEquals(2, editor.newlines)
        assertNotEquals(first, editor.revision)
        assertTrue(editor.edited)
        // A caret move is not an edit, and keeps the very same string.
        val shown = editor.value.text
        assertNull(editor.edit(TextFieldValue(String(shown.toCharArray()), TextRange(0))))
        assertTrue(editor.value.text === shown)
        // The text handed back by the pane is taken as the one typed.
        val revision = editor.revision
        editor.sync(shown)
        assertEquals(revision, editor.revision)
        // From the host, it is counted and named afresh.
        editor.sync("x")
        assertEquals(0, editor.newlines)
        assertNotEquals(revision, editor.revision)
        // Keys compare by revision: equal texts typed apart are two colourings, and nothing hashes the text.
        val ink = CodeInk(Color.White, xdSyntax(MinimalThemePreset.DARK))
        val language = com.restartfu.xd.syntax.SyntaxLanguage.NONE
        assertEquals(EditorKey("ab", 7, language, ink), EditorKey("ab", 7, language, ink))
        assertNotEquals(EditorKey("ab", 7, language, ink), EditorKey("ab", 8, language, ink))
    }

    @Test fun largeFilesAreEditedPlainSooner() {
        assertFalse(isLargeFile(LARGE_FILE_CHARACTERS - 1, LARGE_FILE_LINES - 1))
        assertTrue(isLargeFile(LARGE_FILE_CHARACTERS, 0))
        assertTrue(isLargeFile(0, LARGE_FILE_LINES))
        assertTrue("plain past ${LARGE_FILE_CHARACTERS}", LARGE_FILE_CHARACTERS <= 16 * 1024)
        assertTrue("plain past $LARGE_FILE_LINES lines", LARGE_FILE_LINES <= 600)
    }

    // --- Helpers ---

    private val refusingDelete: (String?, JsonObject) -> String? = { op, _ ->
        if (op == "delete-chat") """{"ok":false,"error":"$REFUSAL"}""" else null
    }

    private var content: (@Composable () -> Unit)? = null

    /** Shows [content] in the activity as it is now; [recreate] shows it again in the next one. */
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
        theme: MinimalThemePreset = MinimalThemePreset.DARK,
        /** The activity's whole app, fatal screen and lifecycle included. */
        whole: Boolean = false,
        steps: () -> Unit,
    ) {
        val activity = compose.activity
        val scripted = AuditHost(override = override).connect().also { host = it }
        model = MainViewModel(activity.application, scripted.client)
        val settings = MobileSettings(activity)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        launch {
            XdRoot(theme) {
                if (whole) {
                    XdMobileApp(model, settings)
                } else {
                    val live by model.client.link.collectAsStateWithLifecycle()
                    MinimalMobileApp(model, live, settings)
                }
            }
        }
        await { model.client.tree.value.folders.isNotEmpty() }
        idle()
        steps()
    }

    private fun goOffline(scripted: AuditHost) {
        scripted.offline = true
        scripted.drop(SocketFailureKind.IO, "The network went away")
        awaitLooper { scripted.client.link.value !is Link.Up }
    }

    private fun draftsSent(scripted: AuditHost, chat: String, text: String): Int = scripted.sent.count {
        it.text("op") == "set-draft" && it.text("chat") == chat && it.text("text") == text
    }

    private fun composer() = compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onLast()

    /** Where the node saying [label] sits in the latest dialog's window, and that window's bounds. */
    private fun dialogBounds(label: String): Pair<Rect, Rect> {
        // Drawn first, so the dialog's layout has caught up.
        ProjectsScreenshotTest.Screen(compose, compose.activity).capture()
        val dialog: Dialog = checkNotNull(ShadowDialog.getLatestDialog()) { "no dialog" }
        fun find(view: View): View? = when (view) {
            is ViewRootForTest -> view
            is ViewGroup -> (0 until view.childCount).firstNotNullOfOrNull { find(view.getChildAt(it)) }
            else -> null
        }
        val root = checkNotNull(find(dialog.window!!.decorView) as? RootForTest).semanticsOwner.unmergedRootSemanticsNode
        fun walk(node: SemanticsNode): List<SemanticsNode> = listOf(node) + node.children.flatMap(::walk)
        val node = walk(root).firstOrNull { node ->
            node.config.getOrNull(SemanticsProperties.Text).orEmpty().any { it.text == label } ||
                node.config.getOrNull(SemanticsProperties.ContentDescription).orEmpty().contains(label)
        }
        return root.boundsInRoot to checkNotNull(node) { "no $label in the dialog" }.boundsInRoot
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

    /** Waits with no composition to advance: only the main looper and the host's threads. */
    private fun awaitLooper(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(50))
            Thread.sleep(10)
        }
        check(condition()) { "Timed out" }
    }

    private fun px(size: Dp): Float = with(compose.density) { size.toPx() }

    private companion object {
        const val TABLET = "w1280dp-h800dp-320dpi"
        const val REFUSAL = "The host could not delete the chat."
        const val DRAFT = "Typed on the train, with no signal"

        fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content
    }
}
