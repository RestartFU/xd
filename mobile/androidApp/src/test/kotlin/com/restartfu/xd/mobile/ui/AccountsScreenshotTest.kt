package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.UsageState
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import com.restartfu.xd.protocol.UsageAccountReply
import com.restartfu.xd.protocol.UsageWindowReply
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
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
 * Several assistant accounts against a scripted host: the accounts dialog,
 * adding one, removing one, a host from before accounts, the notice a chat
 * shows when it moves to another account, and the limits of each account.
 * With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class AccountsScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)
    private var scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @Test fun accountsDark() = accounts(MinimalThemePreset.DARK, Script.ACCOUNTS) {
        check(shows("dana@example.com") && shows("dana.work@example.com"))
        check(shows("Out of its 5-hour limit · back in 2 h 5 min"))
        check(shows("In use") && shows("Add account"))
        check(shows("Switch accounts when one runs out"))
        capture("accounts")
        scrollDialog(10_000f)
        capture("accounts-more")
        scrollDialog(-10_000f)
        compose.onNodeWithText("Remove").performClick()
        settle()
        check(shows("Signs it out and deletes its saved sign-in from this host.") && shows("Keep"))
        capture("remove-confirm")
    }

    @Test fun accountsLight() = accounts(MinimalThemePreset.LIGHT, Script.ACCOUNTS) {
        check(shows("Out of its 5-hour limit · back in 2 h 5 min"))
        capture("accounts-light")
    }

    @Test fun addingAnAccount() = accounts(MinimalThemePreset.DARK, Script.ADDING) {
        check(shows("Add a Codex account") && shows("WXYZ-9876"))
        check(shows("Open sign-in page") && shows("Copy code") && shows("Copy link"))
        scrollDialog(10_000f)
        capture("add-account")
    }

    /** A host from before accounts: one row from the top-level sign-in, and nothing to add. */
    @Test fun olderHost() = accounts(MinimalThemePreset.DARK, Script.OLDER) {
        check(shows("Codex sign-in") && shows("Claude Code sign-in"))
        check(!shows("Add account") && !shows("Switch accounts when one runs out"))
        check(shows("Sign-in and the assistant command-line tools on the connected host."))
        capture("accounts-older-host")
    }

    @Test fun chatMovesToAnotherAccount() {
        val client = XdClient({ ScriptedSocket(Script.ACCOUNTS) }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(ChatSummary("chat", "folder", "Fix reconnect backoff", "codex", false, sessionId = "s"))
        compose.mainClock.autoAdvance = false
        setContent(MinimalThemePreset.DARK) {
            ChatScreen(
                model = chat,
                goBack = {},
                sessionTitle = "reconnect-backoff",
                projectName = "xd",
                backgroundTasks = 0,
                sessionChats = SessionChats(chats, "chat", {}, {}, {}, {}, {}),
            )
        }
        val deadline = System.currentTimeMillis() + 30_000
        while (chat.state.value.visibleItems.size < MESSAGES.size && System.currentTimeMillis() < deadline) settle()
        settle()
        check(shows(SWITCH_SENTENCE))
        // A notice, not a tool call: no run of tool calls swallows it.
        check(!shows("3 tool calls"))
        capture("chat-switch")
    }

    @Test fun limitsOfEachAccount() {
        val client = XdClient({ ScriptedSocket(Script.ACCOUNTS) }, NoCredentials, scope)
        val model = UsageViewModel(client, MobileSettings(compose.activity))
        compose.mainClock.autoAdvance = false
        setContent(MinimalThemePreset.DARK) { UsageContent(model) }
        settle()
        val field = UsageViewModel::class.java.getDeclaredField("_state").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val state = field.get(model) as MutableStateFlow<UsageState>
        state.value = UsageState(metric = "Limits", accounts = LIMITS)
        settle()
        check(shows("Codex · account 1") && shows("Codex · account 2") && shows("Claude"))
        capture("usage-limits")
    }

    private fun accounts(theme: MinimalThemePreset, script: Script, steps: () -> Unit) {
        val model = MainViewModel(compose.activity.application, XdClient({ ScriptedSocket(script) }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        compose.mainClock.autoAdvance = false
        setContent(theme) { AssistantAccountsDialog(model, onDismiss = {}) }
        val deadline = System.currentTimeMillis() + 30_000
        while ((model.agentAuth.value == null || !shows("Command-line tools")) && System.currentTimeMillis() < deadline) settle()
        settle()
        try {
            steps()
        } finally {
            scope.cancel()
        }
    }

    private fun setContent(theme: MinimalThemePreset, content: @Composable () -> Unit) {
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { content() }
                }
            }
        }
    }

    private fun settle() {
        repeat(20) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(10)
        }
        ShadowLooper.idleMainLooper()
    }

    /**
     * Scrolls the dialog by [pixels] with its scroll action. performScrollTo
     * never settles here: with the clock held, the dialog's scroll does not
     * land before it looks again.
     */
    private fun scrollDialog(pixels: Float) {
        val node = compose.onNode(hasScrollAction()).fetchSemanticsNode()
        compose.runOnUiThread { node.config[SemanticsActions.ScrollBy].action?.invoke(0f, pixels) }
        settle()
    }

    private fun shows(text: String) =
        compose.onAllNodes(hasText(text), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    /** Draws the activity and any dialog windows above it, dimmed as the system would. */
    private fun capture(name: String) {
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        val canvas = android.graphics.Canvas(bitmap)
        view.draw(canvas)
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val views = (field.get(instance) as List<android.view.View>).toList()
        for (dialog in views.drop(views.indexOf(view) + 1)) {
            if (dialog.width == 0) continue
            canvas.drawColor(android.graphics.Color.argb(82, 0, 0, 0))
            canvas.save()
            canvas.translate((view.width - dialog.width) / 2f, (view.height - dialog.height) / 2f)
            dialog.draw(canvas)
            canvas.restore()
        }
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
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

    private enum class Script { ACCOUNTS, ADDING, OLDER }

    /** Answers each request from a fixed script, echoing its request id. No action reaches a real host. */
    private class ScriptedSocket(private val script: Script) : PlatformSocket {
        private var listener: PlatformSocketListener? = null

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val body = reply(request["op"]?.jsonPrimitive?.content).replace("\n", "")
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Fix reconnect backoff","backend":"codex","working":false}]}"""
            "agent-auth" -> agentAuth(script)
            "agent-cli-updates" -> """{"ok":true,"providers":[""" +
                """{"provider":"codex","display_name":"Codex","state":"idle","available":false,"updatable":true,"version":"0.98.0"},""" +
                """{"provider":"claude","display_name":"Claude Code","state":"idle","available":false,"updatable":true,"version":"2.4.1"}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"codex","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"label":"Codex","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"gpt-6-sol","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":${MESSAGES.size},"last_message_id":${MESSAGES.size},"messages":[""" +
                MESSAGES.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i * 40}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"codex","name":"Codex",""" +
                """"default_model":"gpt-6-sol","models":[{"id":"gpt-6-sol","name":"GPT-6 Sol"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }

        private fun agentAuth(script: Script): String {
            val now = System.currentTimeMillis()
            // Half a minute short, so the dialog clock, read a moment earlier, still says 2 h 5 min.
            val back = now + 2 * 3_600_000 + 5 * 60_000 - 30_000
            if (script == Script.OLDER) {
                return """{"ok":true,"providers":[""" +
                    """{"provider":"codex","display_name":"Codex","state":"signed-in","needs_input":false},""" +
                    """{"provider":"claude","display_name":"Claude Code","state":"signed-in","needs_input":false}]}"""
            }
            val adding = script == Script.ADDING
            val pending = if (adding) {
                "," + """{"id":"a2","default":false,"pending":true,"label":"Codex account 3","state":"signing-in",""" +
                    """"login_url":"https://auth.example.com/device","device_code":"WXYZ-9876","needs_input":false,""" +
                    """"in_use":false,"busy":true,"windows":[]}"""
            } else ""
            return """{"ok":true,"multiple_accounts":true,"providers":[""" +
                """{"provider":"codex","display_name":"Codex","state":"signed-in","needs_input":false,""" +
                """"auto_switch":true,"adding":$adding,"accounts":[""" +
                """{"id":"default","default":true,"pending":false,"label":"dana@example.com","email":"dana@example.com",""" +
                """"plan":"Pro","state":"signed-in","needs_input":false,"in_use":false,"busy":false,""" +
                """"limited_until_ms":$back,"limited_reason":"5-hour limit","checked_at_ms":$now,"windows":[""" +
                """{"kind":"session","label":"5h","used_percent":100.0,"resets_at_ms":$back},""" +
                """{"kind":"weekly","label":"Weekly","used_percent":59.0}]},""" +
                """{"id":"a1","default":false,"pending":false,"label":"dana.work@example.com","email":"dana.work@example.com",""" +
                """"plan":"Plus","state":"signed-in","needs_input":false,"in_use":true,"busy":false,"checked_at_ms":$now,"windows":[""" +
                """{"kind":"session","label":"5h","used_percent":22.0},{"kind":"weekly","label":"Weekly","used_percent":35.0}]}""" +
                pending + "]}," +
                """{"provider":"claude","display_name":"Claude Code","state":"signed-in","needs_input":false,""" +
                """"auto_switch":true,"adding":false,"accounts":[""" +
                """{"id":"default","default":true,"pending":false,"label":"dana@example.com","email":"dana@example.com",""" +
                """"plan":"Max","state":"signed-in","needs_input":false,"in_use":true,"busy":false,"checked_at_ms":$now,"windows":[""" +
                """{"kind":"session","label":"5h","used_percent":24.0},{"kind":"weekly","label":"Weekly","used_percent":61.0}]}]}]}"""
        }
    }

    private companion object {
        const val SWITCH_SENTENCE = "Switched to dana.work@example.com: dana@example.com reached its 5-hour limit " +
            "(back in 2 h 5 min). Codex sessions cannot move between accounts, so it started a new one with " +
            "this chat's messages. Continuing there."

        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off, and keep the chat usable while it retries.",
            "assistant" to "I'll start with the connection actor, since that's where retries are scheduled.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "Grep \"reconnect\" in shared/src",
            "tool" to "account_switch\n$SWITCH_SENTENCE",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt",
            "assistant" to "Found it. `ConnectionActor` retries on a fixed 500 ms timer and never resets after a good connection. " +
                "I'll replace that with capped exponential backoff with jitter, and reset the delay once the tree loads.",
        )

        val LIMITS = listOf(
            UsageAccountReply(provider = "codex", accountKey = "fixture-codex-1", plan = "Pro", accountRank = 1, windows = listOf(
                UsageWindowReply("primary", "session", "Session", 100.0, resetsAtLabel = "today at 18:05"),
                UsageWindowReply("secondary", "weekly", "Weekly", 59.0, resetsAtLabel = "Friday at 09:00"),
            )),
            UsageAccountReply(provider = "codex", accountKey = "fixture-codex-2", plan = "Plus", accountRank = 2, windows = listOf(
                UsageWindowReply("primary", "session", "Session", 22.0, resetsAtLabel = "today at 20:40"),
                UsageWindowReply("secondary", "weekly", "Weekly", 35.0, resetsAtLabel = "Sunday at 11:00"),
            )),
            UsageAccountReply(provider = "claude", accountKey = "fixture-claude", plan = "Max", windows = listOf(
                UsageWindowReply("primary", "session", "Session", 24.0, resetsAtLabel = "today at 20:00"),
                UsageWindowReply("secondary", "weekly", "Weekly", 61.0, resetsAtLabel = "Saturday at 09:00"),
            )),
        )
    }
}
