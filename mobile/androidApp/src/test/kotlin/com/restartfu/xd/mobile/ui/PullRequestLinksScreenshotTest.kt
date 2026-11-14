package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.view.View
import android.view.ViewGroup
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.LinkedPullRequest
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * The chat header's linked pull requests and the Pull requests list, against
 * a scripted host. With `-Pxd.shots=<dir>` it also writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class PullRequestLinksScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)

    /** Two chips fit beside the session's name; the rest are counted. */
    @Test fun headerShowsTheFirstLinksAndCountsTheRest() {
        val screen = open(LINKED)
        check(shows("#205") && shows("#212") && shows("+3")) { "The header shows two chips and +3" }
        check(!shows("#219")) { "The third link is counted, not shown" }
        capture("android-after-chrome")
        screen.close()
    }

    /**
     * In a phone header the links are a line of small text inside the title's
     * button, so they are read there and opened from More, which also opens
     * the full list.
     */
    @Test fun moreMenuOpensEachLinkAndTheList() {
        val screen = open(LINKED)
        // Nothing in the header's line of links takes a tap of its own.
        for (label in listOf("Pull request #205, merged", "Pull request #212, open", "3 more pull requests")) {
            check(opens(label) == "Switch chat") { "\"$label\" is a button of its own in the header" }
        }
        click("More")
        // The first menu a test opens is never painted, so it opens twice.
        back()
        click("More")
        check(shows("Pull requests")) { "The menu offers Pull requests" }
        check(shows("Open pull request #205") && shows("Open pull request #212")) { "The menu opens the links the header names" }
        check(!shows("Open pull request #219")) { "The menu opens only the links the header names" }
        capture("android-after-menu")
        click("Open pull request #212")
        val opened = org.robolectric.Shadows.shadowOf(compose.activity.application).nextStartedActivity
        check(opened?.dataString == "https://github.com/restartfu/xd/pull/212") { "Opened ${opened?.dataString}" }
        click("More")
        click("Pull requests")
        check(!shows("No pull requests linked yet.") && shows("restartfu/xd") && shows("Merged") && shows("Closed"))
        check(shows("Paste a pull request link") && shows("acme/web")) { "The list holds every link" }
        capture("android-after-dialog")
        click("Close")
        check(!shows("Paste a pull request link")) { "Close dismisses the list" }
        screen.close()
    }

    /** The label of the click action that a tap on what says [text] reaches. */
    private fun opens(text: String): String? {
        var found: SemanticsNode? = nodes().lastOrNull { it.says(text) } ?: error("Nothing shows \"$text\"")
        while (found != null && found.config.getOrNull(SemanticsActions.OnClick) == null) found = found.parent
        return found?.config?.get(SemanticsActions.OnClick)?.label
    }

    @Test fun anEmptyListSaysSo() {
        val screen = open(emptyList())
        click("More")
        back()
        click("More")
        click("Pull requests")
        check(shows("No pull requests linked yet.") && shows("Paste a pull request link"))
        capture("android-after-dialog-empty")
        screen.close()
    }

    /** Linking sends every pasted address; what the host refuses stays with an error. */
    @Test fun refusedLinksStayInTheFieldWithAnError() {
        val screen = open(emptyList())
        click("More")
        back()
        click("More")
        click("Pull requests")
        screen.chat.editPullRequestDraft("https://github.com/restartfu/xd/pull/230 not-a-link")
        screen.chat.linkPullRequests()
        waitFor { !screen.chat.pullRequestLinks.value.linking }
        val sent = screen.sent.single { it["op"]?.jsonPrimitive?.content == "link-pull-requests" }
        check(sent["chat"]!!.jsonPrimitive.content == "chat")
        check(sent["urls"]!!.jsonArray.map { it.jsonPrimitive.content } == listOf("https://github.com/restartfu/xd/pull/230", "not-a-link"))
        val links = screen.chat.pullRequestLinks.value
        check(links.draft == "not-a-link" && links.error == NOT_A_PULL_REQUEST_LINK) { links.toString() }
        idle()
        check(shows(NOT_A_PULL_REQUEST_LINK)) { "The error shows under the field" }
        capture("android-after-dialog-invalid")
        screen.chat.unlinkPullRequest("https://github.com/restartfu/xd/pull/205")
        waitFor { screen.chat.pullRequestLinks.value.removing.isEmpty() }
        check(screen.sent.any { it["op"]?.jsonPrimitive?.content == "unlink-pull-request" && it["url"]?.jsonPrimitive?.content == "https://github.com/restartfu/xd/pull/205" })
        screen.close()
    }

    /** An older host keeps the branch's one pull request and offers no list. */
    @Test fun anOlderHostKeepsTheBranchPullRequest() {
        val screen = open(null)
        waitFor { screen.chat.pullRequest.value != null }
        idle()
        check(shows("#212")) { "The branch's pull request shows" }
        click("More")
        back()
        click("More")
        check(shows("Rename chat") && !shows("Pull requests")) { "No Pull requests item on an older host" }
        check(shows("Open pull request #212")) { "The menu opens the branch's pull request" }
        screen.close()
    }

    private class Screen(val chat: ChatViewModel, val sent: List<JsonObject>, private val scope: CoroutineScope) {
        fun close() = scope.cancel()
    }

    private fun open(linked: List<LinkedPullRequest>?): Screen {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val sent = CopyOnWriteArrayList<JsonObject>()
        val client = XdClient({ ScriptedSocket(sent) }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(
            ChatSummary("chat", "folder", "Fix reconnect backoff", "claude", false, sessionId = "s"),
            ChatSummary("chat2", "folder", "Release notes", "codex", false, sessionId = "s"),
        )
        val theme = MinimalThemePreset.DARK
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = "reconnect-backoff",
                            projectName = "xd",
                            branch = "reconnect-backoff",
                            sessionChats = SessionChats(chats, "chat", {}, {}, {}, {}, {}),
                            linkedPullRequests = linked,
                        )
                    }
                }
            }
        }
        idle()
        return Screen(chat, sent, scope)
    }

    private fun idle() = repeat(30) {
        compose.mainClock.advanceTimeBy(100)
        ShadowLooper.idleMainLooper()
        windows().filter { it.width > 0 && it.height > 0 }.forEach { view ->
            view.draw(android.graphics.Canvas(Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)))
        }
        Thread.sleep(20)
    }

    private fun waitFor(condition: () -> Boolean) {
        repeat(100) {
            if (condition()) return
            idle()
        }
        check(condition()) { "Timed out" }
    }

    /** Closes the open menu, as ProjectsScreenshotTest does. */
    private fun back() {
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        idle()
    }

    private fun windows(): List<View> {
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val all = (field.get(instance) as List<View>).toList()
        // Earlier tests' windows can linger; only those after this activity's own are its.
        return all.drop(all.indexOf(compose.activity.window.decorView).coerceAtLeast(0)).filter { it.isAttachedToWindow }
    }

    private fun nodes(): List<SemanticsNode> = windows().flatMap(::composeViews)
        .flatMap { descendants(it.semanticsOwner.unmergedRootSemanticsNode) }

    private fun SemanticsNode.says(text: String) =
        config.getOrNull(SemanticsProperties.Text)?.any { it.text == text } == true ||
            config.getOrNull(SemanticsProperties.ContentDescription)?.any { it == text } == true ||
            config.getOrNull(SemanticsProperties.EditableText)?.text == text

    private fun shows(text: String) = nodes().any { it.says(text) } ||
        // A placeholder is merged into its field.
        nodes().any { node -> node.config.getOrNull(SemanticsProperties.Text)?.any { it.text.contains(text) } == true }

    private fun click(text: String) {
        var found: SemanticsNode? = nodes().lastOrNull { it.says(text) } ?: error("Nothing shows \"$text\"")
        while (found != null && found.config.getOrNull(SemanticsActions.OnClick) == null) found = found.parent
        val node = checkNotNull(found) { "\"$text\" cannot be clicked" }
        compose.runOnUiThread { check(node.config[SemanticsActions.OnClick].action?.invoke() == true) }
        idle()
    }

    private fun descendants(node: SemanticsNode): List<SemanticsNode> =
        listOf(node) + node.children.flatMap(::descendants)

    private fun composeViews(view: View): List<ViewRootForTest> = when (view) {
        is ViewRootForTest -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { composeViews(view.getChildAt(it)) }
        else -> emptyList()
    }

    /** The activity with any menu or dialog over it drawn where it sits. */
    private fun capture(name: String) {
        val dir = out ?: return
        idle()
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val all = (field.get(instance) as List<android.view.View>).toList()
        val screen = compose.activity.window.decorView
        val views = all.drop(all.indexOf(screen).coerceAtLeast(0))
        val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
        val canvas = android.graphics.Canvas(bitmap)
        screen.draw(canvas)
        for (view in views) {
            if (view === screen || view.width == 0) continue
            val dialog = (view.layoutParams as? android.view.WindowManager.LayoutParams)?.type ==
                android.view.WindowManager.LayoutParams.TYPE_APPLICATION
            if (dialog) canvas.drawColor(android.graphics.Color.argb(82, 0, 0, 0))
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
        dir.mkdirs()
        File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
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

    /** Answers each request from a fixed script, echoing its request id, and keeps what was sent. */
    private class ScriptedSocket(private val sent: MutableList<JsonObject>) : PlatformSocket {
        private var listener: PlatformSocketListener? = null

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                sent += request
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = reply(request).replace("\n", "").removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                val token = request["request"]?.jsonPrimitive?.content
                Thread {
                    Thread.sleep(5)
                    listener?.onBytes((body + "\n").encodeToByteArray())
                    if (op == "git-state" && token != null) {
                        Thread.sleep(5)
                        listener?.onBytes(("""{"event":"git-state","request":"$token","pull_request":""" +
                            """{"branch":"reconnect-backoff","url":"https://github.com/restartfu/xd/pull/212"}}""" + "\n").encodeToByteArray())
                    }
                }.start()
            }
        }

        override fun close() = Unit

        private fun reply(request: JsonObject): String = when (request["op"]?.jsonPrimitive?.content) {
            "tree" -> """{"ok":true,"linked_pull_requests":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Fix reconnect backoff","backend":"claude","working":false}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"label":"Claude","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":2,"last_message_id":2,"messages":[""" +
                """{"id":1,"role":"user","content":"Make reconnect back off after a network drop.","at":1762500000},""" +
                """{"id":2,"role":"assistant","content":"Done. Backoff starts at 500 ms and doubles to a 30 s cap. I opened a pull request for it.","at":1762500001}]}"""
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            "link-pull-requests" -> {
                val urls = request["urls"]!!.jsonArray.map { it.jsonPrimitive.content }
                val (linked, invalid) = urls.partition { "/pull/" in it }
                """{"ok":true,"linked":[${linked.joinToString(",") { "\"$it\"" }}],"invalid":[${invalid.joinToString(",") { "\"$it\"" }}]}"""
            }
            "unlink-pull-request" -> """{"ok":true,"changed":true}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        /** Oldest first, as the host lists them. */
        val LINKED = listOf(
            LinkedPullRequest("https://github.com/restartfu/xd/pull/205", "merged"),
            LinkedPullRequest("https://github.com/restartfu/xd/pull/212", "open"),
            LinkedPullRequest("https://github.com/restartfu/xd/pull/219", "closed"),
            LinkedPullRequest("https://github.com/acme/web/pull/48"),
            LinkedPullRequest("https://github.com/restartfu/xd/pull/224", "open"),
        )
    }
}
