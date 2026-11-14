package com.restartfu.xd.mobile.ui

import android.content.res.Configuration
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isDialog
import androidx.compose.ui.test.isFocused
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.FilesViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.syntax.SyntaxLanguage
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * The fourth audit's fixes: unsaved edits asked about before their tab
 * closes, the editor coloured off the main thread, control edges at 3:1,
 * pages that say their name and take the focus, Settings results above the
 * keyboard, New session to the reading measure, headers that grow with
 * enlarged text, a Workspace heading never left empty, fields named when
 * they hold text, and "Reply finished" told through a live region.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxFinishTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private var host: AuditHost? = null

    @After fun close() {
        host?.close()
    }

    // --- Closing an edited file ---

    @Test fun closingAnEditedFileAsksFirstAndCancelKeepsIt() = files { files ->
        openReadme(files)
        type("# xd\n\nEdited.\n")
        click("Close README.md")
        await { shows("Discard changes to README.md?") }
        assertTrue(shows("Your edits to this file are not saved."))
        // Still open while it asks.
        assertEquals(1, files.state.value.open.files.size)
        compose.onNode(hasText("Cancel") and hasAnyAncestor(isDialog())).performClick()
        idle()
        assertTrue(!shows("Discard changes to README.md?"))
        assertEquals("# xd\n\nEdited.\n", files.state.value.open.files.single().text)
        // Discard drops the edits with the tab.
        click("Close README.md")
        await { shows("Discard changes to README.md?") }
        compose.onNode(hasText("Discard") and hasAnyAncestor(isDialog())).performClick()
        idle()
        assertTrue(files.state.value.open.files.isEmpty())
        assertEquals(0, sentWrites())
    }

    @Test fun anUneditedFileClosesAtOnce() = files { files ->
        openReadme(files)
        click("Close README.md")
        assertTrue(files.state.value.open.files.isEmpty())
        assertTrue(!shows("Discard changes to README.md?"))
    }

    @Test fun saveWritesTheEditsThenCloses() = files { files ->
        openReadme(files)
        type("# xd\n\nSaved.\n")
        click("Close README.md")
        await { shows("Discard changes to README.md?") }
        compose.onNode(hasText("Save") and hasAnyAncestor(isDialog())).performClick()
        idle()
        await { files.state.value.open.files.isEmpty() }
        assertEquals(1, sentWrites())
    }

    @Test fun aSaveThatFailsKeepsTheFileOpenWithTheReason() {
        val failing = AtomicBoolean(true)
        files(override = { op, request ->
            if (op == "file-browse" && request.text("action") == "write" && failing.get()) {
                """{"ok":false,"error":"The file changed on disk."}"""
            } else {
                null
            }
        }) { files ->
            openReadme(files)
            type("# xd\n\nNot saved.\n")
            click("Close README.md")
            await { shows("Discard changes to README.md?") }
            compose.onNode(hasText("Save") and hasAnyAncestor(isDialog())).performClick()
            idle()
            await { files.state.value.open.files.singleOrNull()?.error != null }
            idle()
            val file = files.state.value.open.files.single()
            assertEquals("# xd\n\nNot saved.\n", file.text)
            assertTrue(file.dirty)
            assertTrue(shows("The file changed on disk."))
        }
    }

    @Test fun theQuestionSurvivesARotation() {
        val scripted = AuditHost().connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val files = FilesViewModel { session }
        val restoration = StateRestorationTester(compose)
        compose.mainClock.autoAdvance = false
        var made = 0
        restoration.setContent {
            remember { ++made }
            XdRoot(MinimalThemePreset.DARK) { Box { FilesPaneContent(files) } }
        }
        settle()
        openReadme(files)
        type("# xd\n\nKept.\n")
        click("Close README.md")
        await { shows("Discard changes to README.md?") }
        compose.mainClock.autoAdvance = true
        restoration.emulateSavedInstanceStateRestore()
        compose.mainClock.autoAdvance = false
        idle()
        assertEquals(2, made)
        await { shows("Discard changes to README.md?") }
    }

    // --- The editor's colours ---

    @Test fun typingAndMovingTheCaretNeverColourOnTheMainThread() {
        val kotlin = "fun main() {\n    val name = \"xd\"\n    println(name)\n}\n"
        files(override = { op, request ->
            when {
                op != "file-browse" -> null
                request.text("action") == "read" -> """{"ok":true,"content":${JsonPrimitive(kotlin)}}"""
                else -> """{"ok":true,"entries":[{"name":"Main.kt","directory":false}]}"""
            }
        }) { files ->
            await { shows("Main.kt") }
            HighlightsOnMain.set(0)
            click("Main.kt")
            await { files.state.value.open.current != null }
            idle()
            // Coloured once it has been read.
            await { keywordColour() != null }
            // A keystroke: the colours already there stay round what changed.
            type(kotlin.replace("\"xd\"", "\"xd!\""))
            assertTrue("fun lost its colour after a keystroke", keywordColour() != null)
            // The caret moving changes nothing.
            compose.onNode(hasSetTextAction(), useUnmergedTree = true)
                .performSemanticsAction(SemanticsActions.SetSelection) { it(3, 3, false) }
            idle()
            await { stringColour("\"xd!\"") != null }
            assertEquals("source coloured on the main thread", 0, HighlightsOnMain.get())
        }
    }

    @Test fun aColouringIsMovedAlongWhatWasTyped() {
        val red = SpanStyle(color = Color.Red)
        val blue = SpanStyle(color = Color.Blue)
        // "fun" red, "\"a\"" blue.
        val before = AnnotatedString(
            "fun x = \"a\"",
            listOf(AnnotatedString.Range(red, 0, 3), AnnotatedString.Range(blue, 8, 11)),
        )
        assertEquals(before, colourFor("fun x = \"a\"", before))
        val typed = colourFor("fun xy = \"a\"", before)
        assertEquals("fun xy = \"a\"", typed.text)
        assertEquals(listOf(0 to 3, 9 to 12), typed.spanStyles.map { it.start to it.end })
        // A span the edit falls inside keeps its two ends.
        val inside = colourFor("fuun x = \"a\"", before)
        assertEquals(listOf(0 to 2, 3 to 4, 9 to 12), inside.spanStyles.map { it.start to it.end })
        // No colouring yet: plain.
        assertTrue(colourFor("fun", null).spanStyles.isEmpty())
        // The same text filtered again (a caret move) is the same result, not worked out again.
        val colouring = EditorColouring(before)
        val text = AnnotatedString("fun xy = \"a\"")
        assertTrue(colouring.filter(text).text === colouring.filter(text).text)
    }

    // --- Control edges ---

    @Test fun controlBordersReachThreeToOneOnEveryGround() {
        for (theme in MinimalThemePreset.entries) {
            val scheme = minimalColors(theme)
            val glass = xdGlass(theme)
            val border = xdSignals(theme).controlBorder
            // Every ground a field or outlined control sits on: pages, panes,
            // the composer's glass, cards, menus and dialogs.
            val grounds = mapOf(
                "background" to scheme.background, "surface" to scheme.surface,
                "surfaceVariant" to scheme.surfaceVariant, "surfaceContainerLowest" to scheme.surfaceContainerLowest,
                "surfaceContainerLow" to scheme.surfaceContainerLow, "surfaceContainer" to scheme.surfaceContainer,
                "surfaceContainerHigh" to scheme.surfaceContainerHigh, "surfaceContainerHighest" to scheme.surfaceContainerHighest,
                "backdrop" to glass.backdrop, "pane" to glass.pane.compositeOver(glass.backdrop),
                "composer" to glass.composer.compositeOver(glass.backdrop),
            )
            for ((name, ground) in grounds) {
                val ratio = contrastRatio(border, ground)
                assertTrue("$theme controlBorder on $name is $ratio:1", ratio >= 3f)
            }
        }
    }

    @Test fun noFieldOrControlIsFramedInAHairlineColour() {
        val sources = java.io.File("src/main/kotlin/com/restartfu/xd/mobile/ui").walkTopDown()
            .filter { it.extension == "kt" }.associate { it.name to it.readText() }
        assertTrue(sources.isNotEmpty())
        val quiet = Regex("""glassEdge\(\)|outlineVariant""")
        // Each Material outlined field takes the control edge, and none sets a quiet one.
        val fields = sources.flatMap { (name, text) -> calls(text, "OutlinedTextField(").map { name to it } }
        assertTrue(fields.size > 10)
        for ((name, call) in fields) {
            assertTrue("$name: an OutlinedTextField without xdFieldColors: ${call.take(80)}", "xdFieldColors(" in call)
            assertTrue("$name: an OutlinedTextField with a quiet border", !quiet.containsMatchIn(call))
        }
        assertTrue(!Regex("""BorderColor = (glassEdge\(\)|MaterialTheme\.colorScheme\.outlineVariant)""").containsMatchIn(sources.values.joinToString()))
        // The frames drawn by hand round fields and outlined controls: the composers,
        // the view button, the quota field, and the choice tiles.
        val frames = mapOf(
            "ChatScreen.kt" to 2, "AgentView.kt" to 1, "ChatChrome.kt" to 1, "ChatQuotaViews.kt" to 1,
            "AgentChatRows.kt" to 1, "NewSessionScreen.kt" to 1, "MinimalMobileApp.kt" to 1,
        )
        for ((name, count) in frames) {
            val uses = Regex("""controlBorder\(\)""").findAll(sources.getValue(name)).count()
            assertTrue("$name frames $uses controls in controlBorder, not $count", uses >= count)
        }
        val composers = Regex("""border = BorderStroke\(1\.dp, glassEdge\(\)\)""").findAll(sources.getValue("ChatScreen.kt")).count()
        assertEquals("a composer framed in the pane hairline", 0, composers)
    }

    // --- Pages that say their name ---

    @Test fun eachPageSaysItsNameAndTakesTheFocusOnItsHeading() = app {
        page("Projects")
        click("Terminal")
        page("Terminal")
        click("Usage")
        page("Usage")
        click("Projects")
        page("Projects")
        click("Settings")
        page("Settings")
        click("Appearance")
        page("Appearance")
        pressBack()
        // Back on the home, which says its name; the focus goes back to the
        // section that was opened, not the heading (see UxSeventhTest).
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, "Settings")) }
        idle()
        assertTrue(compose.onAllNodes(isFocused()).fetchSemanticsNodes().single().texts().any { it.startsWith("Appearance") })
        pressBack()
        // Back under Settings, the page beneath is found again.
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, "Projects")) }
        open("Reconnect backoff")
        page("Fix reconnect backoff")
        click("Workspace view: Chat")
        click("Files")
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, "Files")) }
    }

    @Test fun newSessionSaysItsName() = app {
        click("New session in xd")
        repeat(4) { idle() }
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, "New session")) }
    }

    @Test fun theImageViewerSaysItsName() {
        content { ImageViewerContent(ImageBitmap(320, 180), "Image curve.png") {} }
        idle()
        assertTrue(exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, "Image curve.png")))
    }

    // --- Settings search and the keyboard ---

    @Test fun everySettingsResultCanScrollAboveTheKeyboard() = app {
        click("Settings")
        await { shows("Search settings") }
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onFirst()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("s")) }
        idle()
        showKeyboard(compose.activity, px(KEYBOARD).toInt(), px(NAVIGATION).toInt())
        idle()
        compose.mainClock.autoAdvance = true
        val page = compose.onAllNodes(hasScrollAction()).fetchSemanticsNodes().maxBy { it.size.height }
        compose.onNode(SemanticsMatcher("the page's scroll") { it.id == page.id })
            .performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 100_000f) }
        compose.waitForIdle()
        compose.mainClock.autoAdvance = false
        idle()
        val root = compose.onNode(isRoot()).fetchSemanticsNode().boundsInRoot
        val keyboardTop = root.bottom - px(KEYBOARD) - px(NAVIGATION)
        val texts = compose.onAllNodes(
            SemanticsMatcher.keyIsDefined(SemanticsProperties.Text) and hasAnyAncestor(hasTestTag(SETTINGS_PAGE_TAG)),
            useUnmergedTree = true,
        )
            .fetchSemanticsNodes().filter { it.texts().isNotEmpty() }
        val last = texts.maxBy { it.positionInRoot.y + it.size.height }
        val bottom = last.positionInRoot.y + last.size.height
        assertTrue("${last.texts()} ends at $bottom, under the keyboard at $keyboardTop", bottom <= keyboardTop + 1)
        // Not twice the room: only the page's own bottom margin and the card's sit between.
        assertTrue("${last.texts()} ends ${keyboardTop - bottom}px over the keyboard", bottom >= keyboardTop - px(80.dp))
    }

    // --- New session on a tablet ---

    @Test @Config(qualifiers = "w1280dp-h800dp-160dpi")
    fun newSessionKeepsToTheReadingMeasureOnATablet() = app {
        click("New session in xd")
        repeat(4) { idle() }
        await { exists(hasText("What are you working on?")) }
        val field = compose.onAllNodes(hasSetTextAction() and hasAnyAncestor(isDialog()), useUnmergedTree = true).onFirst()
            .fetchSemanticsNode().boundsInRoot
        val start = compose.onNode(hasText("Start session") and hasClickAction()).fetchSemanticsNode().boundsInRoot
        val root = compose.onAllNodes(isRoot()).fetchSemanticsNodes().maxBy { it.boundsInRoot.width }.boundsInRoot
        for ((name, bounds) in listOf("title field" to field, "Start session" to start)) {
            assertTrue("$name is ${bounds.width}px wide", bounds.width <= px(ReadingMeasure) + 1)
            val middle = (bounds.left + bounds.right) / 2
            assertTrue("$name is off the middle: $bounds in $root", kotlin.math.abs(middle - root.center.x) < px(2.dp))
        }
    }

    // --- Headers and enlarged text ---

    @Test @Config(qualifiers = "w1280dp-h800dp-160dpi")
    fun theTabletChatHeaderGrowsWithEnlargedText() = app(fontScale = 2f) {
        open("Reconnect backoff")
        for (label in listOf("Reconnect backoff", "xd")) {
            val node = compose.onAllNodes(hasText(label), useUnmergedTree = true).fetchSemanticsNodes()
                .minBy { it.positionInRoot.y }
            assertTrue("$label is cut: drawn ${node.boundsInRoot.height} of ${node.size.height}px", unclipped(node))
            // Nor squeezed: its lines get all the height they need.
            val layout = mutableListOf<TextLayoutResult>()
            node.config[SemanticsActions.GetTextLayoutResult].action?.invoke(layout)
            val lines = layout.single().multiParagraph.height
            assertTrue("$label needs ${lines}px and has ${layout.single().size.height}", lines <= layout.single().size.height + 1)
        }
        val title = compose.onAllNodes(hasText("Reconnect backoff"), useUnmergedTree = true).fetchSemanticsNodes()
            .minBy { it.positionInRoot.y }
        val project = compose.onAllNodes(hasText("xd"), useUnmergedTree = true).fetchSemanticsNodes()
            .minBy { it.positionInRoot.y }
        // Its lines end above the chat tabs under the header, rather than running under them.
        val tabs = compose.onAllNodes(SemanticsMatcher.expectValue(SemanticsProperties.Role, androidx.compose.ui.semantics.Role.Tab))
            .fetchSemanticsNodes().filter { tab -> tab.texts().any { "Release notes" in it } }.minOf { it.positionInRoot.y }
        val projectBottom = project.positionInRoot.y + project.size.height
        assertTrue("the project's line ends at $projectBottom, under the tabs at $tabs", projectBottom <= tabs + 1)
        val header = compose.onNode(hasTestTag(TABLET_CHAT_HEADER_TAG), useUnmergedTree = true).fetchSemanticsNode()
        val laidOut = project.positionInRoot.y + project.size.height - title.positionInRoot.y
        assertTrue("the title's lines take $laidOut of a ${header.size.height}px header", laidOut <= header.size.height)
        for (node in header.children) assertTrue("${node.texts()} is cut", unclipped(node))
    }

    @Test fun thePhoneHeaderGivesTheSessionNameItsRoomWithEnlargedText() = app(fontScale = 2f) {
        open("Reconnect backoff")
        val subtitle = compose.onNode(hasText("Reconnect backoff · xd"), useUnmergedTree = true).fetchSemanticsNode()
        val layout = mutableListOf<TextLayoutResult>()
        subtitle.config[SemanticsActions.GetTextLayoutResult].action?.invoke(layout)
        val result = layout.single()
        val wanted = result.multiParagraph.intrinsics.maxIntrinsicWidth
        val room = result.layoutInput.constraints.maxWidth
        // The title's button, less its padding, is the line's whole width.
        val line = compose.onNode(SemanticsMatcher("the title's button") { node ->
            node.config.getOrNull(SemanticsActions.OnClick)?.label == "Switch chat"
        }).fetchSemanticsNode().size.width - px(12.dp)
        if (wanted > room) {
            // Cut only when the whole line is not enough: the pull requests gave theirs up first.
            assertTrue("the session's name has $room of the line's $line px", room >= line - 1)
            assertTrue(!exists(hasText("#205")) && !exists(hasText("#212")))
        }
    }

    @Test fun pullRequestLabelsGiveWayFirst() {
        assertEquals(SubtitleLinks.ALL, subtitleLinks(available = 300, subtitle = 150, labels = 100, count = 40, gap = 8))
        assertEquals(SubtitleLinks.COUNT, subtitleLinks(available = 300, subtitle = 220, labels = 100, count = 40, gap = 8))
        assertEquals(SubtitleLinks.NONE, subtitleLinks(available = 300, subtitle = 280, labels = 100, count = 40, gap = 8))
        assertEquals(SubtitleLinks.NONE, subtitleLinks(available = 300, subtitle = 220, labels = 100, count = null, gap = 8))
        assertEquals(SubtitleLinks.NONE, subtitleLinks(available = 300, subtitle = 100, labels = 0, count = null, gap = 8))
    }

    // --- New session for a project with nothing to choose ---

    @Test fun anEmptyWorkspaceHeadingSaysWhy() = app {
        click("New session in xd")
        repeat(4) { idle() }
        await { exists(hasText("Workspace")) }
        await { exists(hasText("Uses the project's own directory", substring = true)) }
    }

    // --- Field names ---

    @Test fun fieldsKeepTheirNamesWithTextInThem() {
        chat {
            compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onLast()
                .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("Look at the backoff")) }
            idle()
            assertTrue(namedField("Message", "Look at the backoff"))
        }
    }

    @Test fun theSettingsSearchAndTheEditorKeepTheirNames() = app {
        click("Settings")
        await { shows("Search settings") }
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onFirst()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("theme")) }
        idle()
        assertTrue(namedField("Search settings", "theme"))
    }

    @Test fun theEditorIsNamedForItsFile() = files { files ->
        openReadme(files)
        type("# xd\n\nEdited.\n")
        assertTrue(namedField("Contents of README.md", "# xd\n\nEdited.\n"))
    }

    // --- Reply finished ---

    @Test fun aFinishedReplyIsToldThroughAPoliteLiveRegion() {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        val working = AtomicBoolean(true)
        val scripted = AuditHost(override = { op, request ->
            if (op == "chat" && request.text("chat") == "c1") {
                AuditHost.reply(op, request).replace("\"working\":false", "\"working\":${working.get()}")
            } else {
                null
            }
        })
        chat(scripted) {
            fun told() = compose.onAllNodes(
                SemanticsMatcher.expectValue(SemanticsProperties.LiveRegion, LiveRegionMode.Polite) and
                    hasContentDescription("Reply finished"),
                useUnmergedTree = true,
            ).fetchSemanticsNodes().isNotEmpty()
            assertTrue(!told())
            working.set(false)
            checkNotNull(host).push("""{"event":"changed","chat":"c1"}""")
            await { told() }
        }
    }

    // --- Helpers ---

    private fun page(title: String) {
        await { exists(SemanticsMatcher.expectValue(SemanticsProperties.PaneTitle, title)) }
        idle()
        // The focus is on the page's heading, which says the same.
        val focused = compose.onAllNodes(isFocused(), useUnmergedTree = true).fetchSemanticsNodes()
        assertTrue(
            "the focus is on ${focused.map { it.texts() }}, not the $title heading",
            focused.any { node -> node.config.contains(SemanticsProperties.Heading) && title in node.texts() },
        )
    }

    private fun unclipped(node: SemanticsNode): Boolean = node.boundsInRoot.height >= node.size.height - 1

    private fun keywordColour(): Color? = layoutText()?.spanStyles?.firstOrNull { it.start == 0 && it.end == 3 }?.item?.color

    private fun stringColour(literal: String): Color? {
        val text = layoutText() ?: return null
        val at = text.text.indexOf(literal)
        return text.spanStyles.firstOrNull { it.start <= at && it.end >= at + literal.length }?.item?.color
    }

    /** What the editor lays out: its text as coloured for drawing. */
    private fun layoutText(): AnnotatedString? {
        val node = compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).fetchSemanticsNodes().firstOrNull() ?: return null
        val layout = mutableListOf<TextLayoutResult>()
        node.config.getOrNull(SemanticsActions.GetTextLayoutResult)?.action?.invoke(layout)
        return layout.firstOrNull()?.layoutInput?.text
    }

    private fun openReadme(files: FilesViewModel) {
        await { shows("README.md") }
        click("README.md")
        await { files.state.value.open.current != null }
        idle()
    }

    /** Puts [text] in the editor, as typing does, without focusing it. */
    private fun type(text: String) {
        compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onFirst()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString(text)) }
        idle()
    }

    private fun sentWrites(): Int = checkNotNull(host).sent.count {
        (it["op"] as? JsonPrimitive)?.content == "file-browse" && (it["action"] as? JsonPrimitive)?.content == "write"
    }

    private fun files(
        override: (String?, JsonObject) -> String? = { _, _ -> null },
        steps: (FilesViewModel) -> Unit,
    ) {
        val scripted = AuditHost(override = override).connect().also { host = it }
        val session = scripted.client.openChat("c1")
        val files = FilesViewModel { session }
        content { FilesPaneContent(files) }
        steps(files)
    }

    private fun shows(label: String): Boolean = compose.onAllNodes(
        hasText(label) or hasContentDescription(label),
        useUnmergedTree = true,
    ).fetchSemanticsNodes().isNotEmpty()

    /**
     * A field that reads [typed] as its text, with [name] in it as a label is,
     * and no content description to be read in the text's place.
     */
    private fun namedField(name: String, typed: String): Boolean =
        compose.onAllNodes(hasSetTextAction() and hasText(name)).fetchSemanticsNodes().any { node ->
            node.config.getOrNull(SemanticsProperties.EditableText)?.text == typed &&
                node.config.getOrNull(SemanticsProperties.ContentDescription).isNullOrEmpty()
        }

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

    private fun app(fontScale: Float = 1f, steps: () -> Unit) {
        val activity = compose.activity
        if (fontScale != 1f) {
            val resources = activity.resources
            val config = Configuration(resources.configuration).apply { this.fontScale = fontScale }
            @Suppress("DEPRECATION")
            resources.updateConfiguration(config, resources.displayMetrics)
        }
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

    private fun chat(scripted: AuditHost = AuditHost(), steps: () -> Unit) {
        scripted.connect().also { host = it }
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

    private companion object {
        val KEYBOARD = 300.dp
        val NAVIGATION = 48.dp

        fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content

        /** The text of each call of [function] in [source], to its closing parenthesis. */
        fun calls(source: String, function: String): List<String> {
            val found = mutableListOf<String>()
            var from = 0
            while (true) {
                val at = source.indexOf(function, from).takeIf { it >= 0 } ?: break
                if (at > 0 && (source[at - 1].isLetterOrDigit() || source[at - 1] == '.')) {
                    from = at + function.length
                    continue
                }
                var depth = 1
                var index = at + function.length
                while (depth > 0 && index < source.length) {
                    when (source[index]) {
                        '(' -> depth++
                        ')' -> depth--
                    }
                    index++
                }
                found += source.substring(at, index)
                from = index
            }
            return found
        }
    }
}
