package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.util.Base64
import android.view.View
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.AndroidComposeTestRule
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.ByteArrayOutputStream
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Rule
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Renders the Projects page against a scripted host that keeps folders.
 *
 * With `-Pxd.shots=<dir>` it writes PNG captures there for design review.
 * Only a project the list keeps closed has a working session: a running
 * turn's pulse in an open project never lets the paused Robolectric clock
 * settle once the list recomposes, while a closed row's dot holds still.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ProjectsScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun projectsDark() = shoot("projects-dark", MinimalThemePreset.DARK)

    @Test fun projectsLight() = shoot("projects-light", MinimalThemePreset.LIGHT)

    @Test fun bottomBarLeavesOutSessions() = shoot("bottom-bar-dark", MinimalThemePreset.DARK) {
        // Sessions open from the Projects page; the phone's bar has no tab for them.
        check(shows("Projects") && shows("Terminal") && shows("Usage") && !shows("Sessions"))
    }

    @Test @Config(qualifiers = "w840dp-h1200dp-320dpi")
    fun railLeavesOutSessions() = shoot("rail-dark", MinimalThemePreset.DARK) {
        check(shows("Projects") && shows("Terminal") && shows("Usage") && !shows("Sessions"))
    }

    @Test @Config(qualifiers = "w840dp-h1200dp-320dpi")
    fun tabletKeepsALabelledNewButton() = shoot("rail-new-menu-dark", MinimalThemePreset.DARK) {
        check(this@ProjectsScreenshotTest.compose.onAllNodes(hasText("New"), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty())
        click("New")
        check(shows("New session") && shows("New project") && shows("New folder"))
    }

    @Test fun projectsOpenOnTheirOwn() = shoot("projects-open-dark", MinimalThemePreset.DARK) {
        // Every project starts open, so sessions in two projects show at once.
        check(shows("Reconnect backoff") && shows("Landing copy") && shows("No sessions yet"))
        click("xd")
        check(!shows("Reconnect backoff") && !shows("Usage page") && shows("Landing copy"))
        click("xd")
        check(shows("Reconnect backoff") && shows("Landing copy"))
    }

    @Test fun newSessionAsksForAProject() = shoot("new-session-picker-dark", MinimalThemePreset.DARK) {
        click("New")
        click("New session")
        check(shows("Which project?") && shows("2 sessions") && shows("acme · 1 session") &&
            shows("acme / Clients · 1 session") && !shows("Start session"))
    }

    @Test fun pickingAProjectOpensItsForm() = shoot("new-session-chosen-dark", MinimalThemePreset.DARK) {
        click("New")
        click("New session")
        val before = SENT.size
        // Nothing waits on the next dialog: its title field never lets Compose go idle.
        // The checkouts it reads say which project it is for.
        click("acme / Clients · 1 session")
        awaitPlaces(before)
        check(placesRead(before) == listOf("shop")) { placesRead(before).toString() }
    }

    @Test fun anOpenProjectsPlusOpensItsForm() = shoot("new-session-here-dark", MinimalThemePreset.DARK) {
        check(shows("New session in web") && shows("New session in xd"))
        val before = SENT.size
        click("New session in web")
        awaitPlaces(before)
        check(placesRead(before) == listOf("web")) { placesRead(before).toString() }
    }

    /**
     * Waits for the form to ask for a project's checkouts, which it does as it
     * opens. Its effects run on the main looper, which Robolectric only runs
     * when told to.
     */
    private fun Screen.awaitPlaces(from: Int) {
        val deadline = System.currentTimeMillis() + 10_000
        while (placesRead(from).isEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** The projects whose checkouts the app asked for since request [from]. */
    private fun placesRead(from: Int): List<String> = synchronized(SENT) { SENT.drop(from) }
        .map { Json.parseToJsonElement(it).jsonObject }
        .filter { it["op"]?.jsonPrimitive?.content == "folder-settings" }
        .mapNotNull { it["folder"]?.jsonPrimitive?.content }

    @Test fun phoneNewIsTheRoundButton() = shoot("phone-new-dark", MinimalThemePreset.DARK) {
        // Named for screen readers, with no visible label and no row of its own above the list.
        check(shows("New") && !shows("4 projects · 2 folders"))
        check(this@ProjectsScreenshotTest.compose.onAllNodes(hasText("New"), useUnmergedTree = true).fetchSemanticsNodes().isEmpty())
    }

    @Test @Config(qualifiers = "w360dp-h780dp-420dpi")
    fun narrowPhone() = longList("narrow-dark", MinimalThemePreset.DARK)

    @Test @Config(qualifiers = "w320dp-h640dp-320dpi")
    fun narrowestPhone() = longList("narrowest-dark", MinimalThemePreset.DARK)

    @Test fun barStaysCentredOnTerminal() = shoot("terminal-centred-dark", MinimalThemePreset.DARK) {
        click("Terminal")
        checkBarCentred()
    }

    @Test fun longListDark() = longList("long-list-dark", MinimalThemePreset.DARK)

    @Test fun longListLight() = longList("long-list-light", MinimalThemePreset.LIGHT)

    @Test fun pausingAProjectStopsOnlyItsChats() = shoot("paused-dark", MinimalThemePreset.DARK) {
        // Only projects with work show the button: web runs a turn, shop waits on a task.
        check(shows("Pause chats in web") && shows("Pause chats in shop"))
        check(!shows("Pause chats in xd") && !shows("Resume chats in web") && !shows("Paused"))
        this@ProjectsScreenshotTest.compose.onNode(hasContentDescription("Pause chats in web")).assertIsEnabled()
        click("Pause chats in web")
        awaitShown("Resume chats in web")
        check(sent("pause-all") == 1) { "pause-all was sent ${sent("pause-all")} times" }
        check(folderOf("pause-all") == "web") { "pause-all went to ${folderOf("pause-all")}" }
        // shop is another project, so it keeps working.
        check(shows("Resume chats in web") && shows("Pause chats in shop") && shows("Paused"))
    }

    @Test fun pausedLight() = shoot("paused-light", MinimalThemePreset.LIGHT, paused = true) {
        check(shows("Resume chats in web") && shows("Resume chats in shop") && shows("Paused"))
    }

    @Test fun resumingAProjectContinuesItsChats() = shoot("resumed-dark", MinimalThemePreset.DARK, paused = true) {
        click("Resume chats in web")
        awaitShown("Pause chats in web")
        check(sent("resume-all") == 1) { "resume-all was sent ${sent("resume-all")} times" }
        check(folderOf("resume-all") == "web") { "resume-all went to ${folderOf("resume-all")}" }
        check(!shows("Resume chats in web") && shows("Resume chats in shop"))
    }

    @Test fun sessionsStartSideBySide() = shoot("starting-dark", MinimalThemePreset.DARK) {
        holdNewChats = true
        val sentBefore = sent("new-chat")
        // A second session starts while the host still sets up the first.
        model.createDirectSession("xd", "Fix the footer", DirectAgent.CLAUDE)
        model.createDirectSession("xd", "Tidy the header", DirectAgent.CODEX)
        awaitSent("new-chat", sentBefore + 2)
        awaitShown("Tidy the header")
        check(shows("Fix the footer") && shows("Tidy the header"))
        val starting = this@ProjectsScreenshotTest.compose
            .onAllNodes(hasText("Starting…"), useUnmergedTree = true).fetchSemanticsNodes().size
        check(starting == 2) { "$starting rows say Starting…" }
        check(model.startingSessions.value.map { it.title } == listOf("Fix the footer", "Tidy the header"))
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "starting-held-dark.png").outputStream().use { capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        // Going somewhere else meanwhile: the made ones do not take its place.
        click("Terminal")
        releaseNewChats()
        val deadline = System.currentTimeMillis() + 10_000
        while (model.startingSessions.value.isNotEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(model.startingSessions.value.isEmpty()) { "still starting: ${model.startingSessions.value}" }
        check(model.createdDirectSession.value == null) { "opened ${model.createdDirectSession.value}" }
        check(shows("Terminal"))
        click("Projects")
        check(!shows("Starting…"))
    }

    @Test fun onlyTheNewestStartedSessionOpens() = shoot("starting-newest-dark", MinimalThemePreset.DARK) {
        holdNewChats = true
        val sentBefore = sent("new-chat")
        model.createDirectSession("xd", "Fix the footer", DirectAgent.CLAUDE)
        model.createDirectSession("xd", "Tidy the header", DirectAgent.CODEX)
        awaitSent("new-chat", sentBefore + 2)
        releaseNewChats()
        val deadline = System.currentTimeMillis() + 10_000
        while (model.startingSessions.value.isNotEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        // The scripted tree never lists the made chats, so the one to open
        // stays unconsumed: it is the second, whichever was made first.
        check(model.createdDirectSession.value?.title == "Tidy the header") {
            "opened ${model.createdDirectSession.value}"
        }
    }

    /** Waits until the app has sent [count] [op] requests. */
    private fun Screen.awaitSent(op: String, count: Int) {
        val deadline = System.currentTimeMillis() + 10_000
        while (sent(op) < count && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(sent(op) == count) { "$op was sent ${sent(op)} times" }
    }

    @Test fun anOlderHostHasNoPauseButton() = shoot("no-pause-dark", MinimalThemePreset.DARK, pauseAll = false) {
        check(shows("Projects") && !shows("Pause chats in web") && !shows("Resume chats in web"))
    }

    /** The folder the last [op] request named. */
    private fun folderOf(op: String): String? = synchronized(SENT) { SENT.toList() }
        .map { Json.parseToJsonElement(it).jsonObject }
        .lastOrNull { it["op"]?.jsonPrimitive?.content == op }
        ?.get("folder")?.jsonPrimitive?.content

    /** How many [op] requests the app sent. */
    private fun sent(op: String): Int = synchronized(SENT) { SENT.toList() }
        .count { Json.parseToJsonElement(it).jsonObject["op"]?.jsonPrimitive?.content == op }

    /** Waits for the host's answer and the tree it refreshes to reach the page. */
    private fun Screen.awaitShown(label: String) {
        val deadline = System.currentTimeMillis() + 10_000
        while (!shows(label) && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    @Test fun terminalTab() = shoot("terminal-dark", MinimalThemePreset.DARK) { click("Terminal") }

    @Test fun usageTab() = shoot("usage-dark", MinimalThemePreset.DARK) { click("Usage") }

    /** A list longer than the screen, to show what the bottom bar covers. */
    private fun longList(name: String, theme: MinimalThemePreset) {
        longTree = true
        try {
            shoot(name, theme) {
                check(shows("Quota bars") && shows("New"))
                // The round button sits above the bar at any width, so neither covers the other.
                val usage = bounds { hasContentDescription("Usage") }
                val button = bounds { hasContentDescription("New") }
                check(button.bottom < usage.top) { "Usage tab $usage, New button $button" }
                checkBarCentred()
            }
        } finally {
            longTree = false
        }
    }

    @Test fun newMenu() = shoot("new-menu-dark", MinimalThemePreset.DARK) {
        showNewMenu()
        check(shows("New session") && shows("New project") && shows("New folder"))
    }

    @Test fun newMenuLight() = shoot("new-menu-light", MinimalThemePreset.LIGHT) {
        showNewMenu()
        check(shows("New session"))
    }

    @Test fun newFolderDialog() = shoot("new-folder-dark", MinimalThemePreset.DARK) {
        click("New")
        // Nothing waits on the dialog itself: one holding a text field never
        // lets Compose go idle under Robolectric. The organization's request
        // is checked in the shared OpsTest and ProjectTreeTest.
        click("New folder")
    }

    @Test fun folderMenu() = shoot("folder-menu-dark", MinimalThemePreset.DARK) {
        // A top-level folder with no other folder to go into cannot move.
        click("acme actions")
        check(shows("New project here") && shows("Prompt shortcuts") && shows("Folder context") &&
            shows("Agent secrets") && shows("Delete folder") && !shows("Move to…"))
        pressBack()
        click("Clients actions")
        check(shows("New project here") && shows("Move to…") && shows("Delete folder"))
    }

    @Test fun moveDialog() = shoot("move-dark", MinimalThemePreset.DARK) {
        click("notes")
        click("notes actions")
        click("Move to…")
        click("Clients")
        check(shows("Here now"))
    }

    @Test fun closingAFolderHidesWhatItHolds() = shoot("folder-closed-dark", MinimalThemePreset.DARK) {
        check(shows("web") && shows("shop"))
        click("acme")
        check(!shows("web") && !shows("shop") && !shows("Clients"))
        click("acme")
        check(shows("web") && shows("shop"))
    }

    @Test fun aClosedFolderShowsWhatItsProjectsNeed() = shoot("folder-closed-signals-dark", MinimalThemePreset.DARK) {
        val working = xdSignals(MinimalThemePreset.DARK).working.toArgb()
        // Open, the folder leaves its projects' dots to their own rows.
        check(paintedOnRowOf("acme", working) == 0) { "an open folder shows dots" }
        click("acme")
        check(!shows("web") && !shows("shop"))
        // web's working session and shop's background task, held in the closed folder.
        check(paintedOnRowOf("acme", working) > 0) { "the closed folder shows no dots" }
    }

    @Test fun movingSendsTheChosenFolder() = shoot("moved-dark", MinimalThemePreset.DARK) {
        click("notes")
        click("notes actions")
        click("Move to…")
        click("Clients")
        click("Move")
        val move = SENT.last { it.contains("\"move-folder\"") }
        check(move.contains("\"folder\":\"notes\"") && move.contains("\"parent\":\"clients\"")) { move }
    }

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** The app's view model, for steps that start what a form would. */
    private lateinit var model: MainViewModel

    /** Opens the page, lets [steps] act on it, and captures the result. */
    private fun shoot(
        name: String,
        theme: MinimalThemePreset,
        paused: Boolean = false,
        pauseAll: Boolean = true,
        steps: Screen.() -> Unit = {},
    ) {
        pausedFolders = if (paused) setOf("web", "shop") else emptySet()
        canPauseAll = pauseAll
        holdNewChats = false
        HELD.clear()
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        model = MainViewModel(activity.application, XdClient({ ScriptedSocket() }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        val settings = MobileSettings(activity)
        // Time moves only when the test says, so running animations cannot keep Compose busy.
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { MinimalMobileApp(model, link, settings) }
                }
            }
        }
        val screen = Screen(compose, activity)
        // The scripted host answers on its own threads, so wait for the tree and avatars.
        val deadline = System.currentTimeMillis() + 60_000
        while (
            (model.client.tree.value.folders.isEmpty() || model.client.ownerIcons.value.size < 2) &&
            System.currentTimeMillis() < deadline
        ) screen.settle()
        screen.settle()
        try {
            screen.steps()
        } catch (failure: Throwable) {
            // What was on screen when it went wrong, to see why.
            out?.let { dir ->
                dir.mkdirs()
                File(dir, "$name-failed.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
            }
            // A dialog holding a text field never lets Compose go idle, so its labels may not be readable.
            val shown = runCatching { screen.labels().toString() }.getOrDefault("(screen not idle)")
            throw AssertionError("${failure.message}; showing $shown", failure)
        }
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        scope.cancel()
    }

    /** Drives the page as a person would by touch. */
    class Screen(
        private val compose: AndroidComposeTestRule<*, *>,
        private val activity: ComponentActivity,
    ) {
        fun settle() {
            repeat(10) {
                Thread.sleep(15)
                compose.mainClock.advanceTimeBy(50)
            }
        }

        private fun matching(label: String) = compose.onAllNodes(
            hasText(label) or hasContentDescription(label),
            useUnmergedTree = true,
        )

        fun labels(): List<String> = compose.onAllNodes(isRoot()).fetchSemanticsNodes().flatMap { root ->
            fun walk(node: SemanticsNode): List<SemanticsNode> = listOf(node) + node.children.flatMap(::walk)
            walk(root)
        }.mapNotNull { node ->
            node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }
                ?: node.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString()
        }

        /** Where the one node [matcher] picks sits on screen, merged with its children. */
        fun bounds(matcher: () -> SemanticsMatcher) = compose.onNode(matcher()).fetchSemanticsNode().boundsInRoot

        /**
         * Opens the New menu so a capture shows it. Under Robolectric the first
         * menu a test opens is never painted into its window, so it opens twice.
         */
        fun showNewMenu() {
            click("New")
            pressBack()
            click("New")
        }

        /** The floating bar sits in the middle of the window. */
        fun checkBarCentred() {
            val root = bounds { isRoot() }
            val left = bounds { hasContentDescription("Projects") }.left - root.left
            val right = root.right - bounds { hasContentDescription("Usage") }.right
            check(kotlin.math.abs(left - right) < 2f) { "bar is $left from the left and $right from the right" }
        }

        fun shows(label: String): Boolean = matching(label).fetchSemanticsNodes().isNotEmpty()

        /** How many pixels of exactly [color] the row holding [label] paints, across the screen's width. */
        fun paintedOnRowOf(label: String, color: Int): Int {
            val bounds = matching(label).fetchSemanticsNodes().last().boundsInWindow
            val bitmap = capture()
            val top = bounds.top.toInt().coerceIn(0, bitmap.height)
            val bottom = bounds.bottom.toInt().coerceIn(top, bitmap.height)
            return (top until bottom).sumOf { y -> (0 until bitmap.width).count { x -> bitmap.getPixel(x, y) == color } }
        }

        fun click(label: String) {
            matching(label).onLast().performClick()
            settle()
        }

        /**
         * Sets a field's text without focusing it: a focused field's blinking
         * cursor never lets Compose go idle under the test clock.
         */


        fun pressBack() {
            compose.runOnUiThread { activity.onBackPressedDispatcher.onBackPressed() }
            ShadowLooper.idleMainLooper()
            settle()
        }

        /** This activity's window and the dialogs and menus over it. */
        private fun windows(): List<View> {
            val global = Class.forName("android.view.WindowManagerGlobal")
            val instance = global.getMethod("getInstance").invoke(null)
            val field = global.getDeclaredField("mViews").apply { isAccessible = true }
            @Suppress("UNCHECKED_CAST")
            val views = (field.get(instance) as List<View>).toList()
            val screen = activity.window.decorView
            // Earlier tests' windows can linger; only those after this activity's own are its.
            return views.drop(views.indexOf(screen).coerceAtLeast(0))
        }

        /** The activity with any dialogs and menus drawn over it where they sit. */
        fun capture(): Bitmap {
            ShadowLooper.idleMainLooper()
            val views = windows()
            val screen = activity.window.decorView
            val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            screen.draw(canvas)
            for (view in views) {
                if (view === screen || view.width == 0) continue
                val params = view.layoutParams as? android.view.WindowManager.LayoutParams
                val dialog = params?.type == android.view.WindowManager.LayoutParams.TYPE_APPLICATION
                if (dialog) canvas.drawColor(Color.argb(82, 0, 0, 0))
                val location = IntArray(2).also(view::getLocationOnScreen)
                val (x, y) = if (dialog) {
                    (screen.width - view.width) / 2f to (screen.height - view.height) / 2f
                } else {
                    location[0].toFloat() to location[1].toFloat()
                }
                canvas.save()
                canvas.translate(x, y)
                view.draw(canvas)
                canvas.restore()
            }
            return bitmap
        }
    }

    private object NoCredentials : CredentialStore {
        override suspend fun load(): StoredCredentials? = null
        override suspend fun save(credentials: StoredCredentials) = Unit
        override suspend fun clear() = Unit
    }

    private val connection = SshConnection(
        "host", 22, "dev", SshAuthentication.Password("x"),
        SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:test"),
    )

    /** Answers each request from a fixed script, echoing its request id. */
    private class ScriptedSocket : PlatformSocket {
        private var listener: PlatformSocketListener? = null

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                SENT += line
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = reply(
                    op,
                    request["owner"]?.jsonPrimitive?.content,
                    request["folder"]?.jsonPrimitive?.content,
                )
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                val answer: () -> Unit = { listener?.onBytes((body + "\n").encodeToByteArray()) }
                // Held as a slow worktree would hold it, until the test lets it go.
                if (op == "new-chat" && holdNewChats) synchronized(HELD) { HELD.add(answer) }
                else Thread { Thread.sleep(5); answer() }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?, owner: String?, folder: String?): String = when (op) {
            "tree" -> (if (longTree) TREE_LONG else TREE)
                .let { paused(it) }
                .let { if (canPauseAll) it else it.replace("\"pause_all\":true,\"pause_folders\":true,", "") }
            "pause-all" -> { pausedFolders = pausedFolders + folder.orEmpty(); """{"ok":true,"paused":[]}""" }
            "resume-all" -> { pausedFolders = pausedFolders - folder.orEmpty(); """{"ok":true,"resumed":[]}""" }
            "owner-icon" -> """{"ok":true,"owner":"$owner","mime":"image/png","data":"${avatar(owner.orEmpty())}"}"""
            "new-chat" -> """{"ok":true,"id":"made-${madeChats.incrementAndGet()}"}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        /** Every request the app sent, in order. */
        /** Whether the scripted host answers with [TREE_LONG]. */
        @Volatile var longTree = false

        /** The projects the scripted host has paused: web's turn and shop's task stop with theirs. */
        @Volatile var pausedFolders = emptySet<String>()

        /** Whether the scripted host says it can pause every chat. */
        @Volatile var canPauseAll = true

        /** Whether the scripted host holds new chats' answers until [releaseNewChats]. */
        @Volatile var holdNewChats = false

        /** The answers held back. */
        val HELD = mutableListOf<() -> Unit>()

        val madeChats = java.util.concurrent.atomic.AtomicInteger()

        fun releaseNewChats() {
            holdNewChats = false
            val held = synchronized(HELD) { HELD.toList().also { HELD.clear() } }
            held.forEach { answer -> Thread { answer() }.start() }
        }

        fun paused(tree: String): String = tree
            .let {
                if ("web" in pausedFolders) it.replace(""""working":true,"session":"s3"}""", """"working":false,"paused":true,"session":"s3"}""")
                else it
            }
            .let {
                if ("shop" in pausedFolders) it.replace(""""background_tasks":1,"session":"s4"}""", """"paused":true,"session":"s4"}""")
                else it
            }

        /** [TREE] with a dozen more sessions, so the list runs past the bottom of the screen. */
        val TREE_LONG by lazy {
            val names = listOf(
                "Pairing flow", "Diff pane", "Terminal resize", "Model picker", "Settle shelf", "Folder moves",
                "Push notices", "Draft sync", "Image viewer", "Theme tokens", "Quota bars", "Host update",
            )
            val sessions = names.mapIndexed { i, title ->
                """{"id":"x$i","folder":"xd","title":"$title","chats":["y$i"],"terminal_chat":"session:x$i"}"""
            }
            val chats = names.mapIndexed { i, title ->
                val backend = if (i % 2 == 0) "claude" else "codex"
                """{"id":"y$i","folder":"xd","title":"$title","backend":"$backend","working":false,"session":"x$i"}"""
            }
            TREE.replace("\"sessions\":[", "\"sessions\":[" + sessions.joinToString(",") + ",")
                .replace("\"chats\":[{", "\"chats\":[" + chats.joinToString(",") + ",{")
        }

        val SENT: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())

        /** An organization holding a project and a folder of client sites, beside two plain projects. */
        val TREE = """{"ok":true,"settlement":true,"folder_reorder":true,"folder_groups":true,"pause_all":true,"pause_folders":true,"folders":[""" +
            """{"id":"xd","name":"xd","owner":"RestartFU"},""" +
            """{"id":"acme","name":"acme","group":true,"owner":"acme"},""" +
            """{"id":"web","name":"web","parent":"acme","owner":"acme"},""" +
            """{"id":"clients","name":"Clients","parent":"acme","group":true},""" +
            """{"id":"shop","name":"shop","parent":"clients"},""" +
            """{"id":"notes","name":"notes"}],""" +
            """"sessions":[{"id":"s1","folder":"xd","title":"Reconnect backoff","chats":["c1"],"terminal_chat":"session:s1"},""" +
            """{"id":"s2","folder":"xd","title":"Usage page","chats":["c2"],"terminal_chat":"session:s2"},""" +
            """{"id":"s3","folder":"web","title":"Landing copy","chats":["c3"],"terminal_chat":"session:s3"},""" +
            """{"id":"s4","folder":"shop","title":"Checkout fix","chats":["c4"],"terminal_chat":"session:s4"}],""" +
            """"chats":[{"id":"c1","folder":"xd","title":"Reconnect backoff","backend":"claude","working":false,"session":"s1"},""" +
            """{"id":"c2","folder":"xd","title":"Usage page","backend":"codex","working":false,"session":"s2"},""" +
            """{"id":"c3","folder":"web","title":"Landing copy","backend":"codex","working":true,"session":"s3"},""" +
            """{"id":"c4","folder":"shop","title":"Checkout fix","backend":"claude","working":false,"background_tasks":1,"session":"s4"}]}"""

        /** A plain stand-in avatar: the account's initial on a colour of its own. */
        fun avatar(owner: String): String {
            val bitmap = Bitmap.createBitmap(64, 64, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            canvas.drawColor(if (owner == "acme") Color.rgb(0x3b, 0x82, 0xf6) else Color.rgb(0xe1, 0x6b, 0x3a))
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                color = Color.WHITE
                textSize = 40f
                textAlign = Paint.Align.CENTER
                isFakeBoldText = true
            }
            canvas.drawText(owner.take(1).uppercase(), 32f, 46f, paint)
            val bytes = ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            return Base64.encodeToString(bytes, Base64.NO_WRAP)
        }
    }
}
