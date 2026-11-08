package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
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
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Renders the host's Titles and names settings against a scripted host, with
 * titles on and off, and checks that each assistant's naming model row is
 * shown either way. With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ChatTitleSettingsScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun titlesOn() = shoot("chat-titles-on", enabled = true) {
        check(shows("Codex naming model") && shows("Claude naming model"))
        check(shows("Titles Codex chats and names their worktrees."))
    }

    /** Worktrees are still named with titles off, so their model rows stay shown. */
    @Test fun titlesOff() = shoot("chat-titles-off", enabled = false) {
        check(shows("Codex naming model") && shows("Claude naming model"))
        check(shows("GPT-6 Luna") && shows("Claude Haiku 5.5"))
    }

    private fun shoot(name: String, enabled: Boolean, steps: () -> Unit) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ ScriptedSocket(enabled) }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        compose.mainClock.autoAdvance = false
        val theme = MinimalThemePreset.DARK
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            Text("Host", style = MaterialTheme.typography.titleSmall)
                            ChatTitleSettings(model)
                        }
                    }
                }
            }
        }
        val deadline = System.currentTimeMillis() + 30_000
        // Until the page itself shows the settings: the model can hold them a
        // frame before the screen does.
        while (
            (model.hostCatalog.value.isEmpty() || !shows("Generate chat titles")) &&
            System.currentTimeMillis() < deadline
        ) settle()
        settle()
        try {
            steps()
        } finally {
            out?.let { dir ->
                dir.mkdirs()
                val screen = activity.window.decorView
                val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
                screen.draw(Canvas(bitmap))
                File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
            }
            scope.cancel()
        }
    }

    private fun settle() {
        repeat(10) {
            Thread.sleep(15)
            ShadowLooper.idleMainLooper()
            compose.mainClock.advanceTimeBy(50)
        }
    }

    private fun shows(text: String) =
        compose.onAllNodes(hasText(text), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

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
    private class ScriptedSocket(private val enabled: Boolean) : PlatformSocket {
        private var listener: PlatformSocketListener? = null

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val body = reply(request["op"]?.jsonPrimitive?.content)
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[],"sessions":[],"chats":[]}"""
            "chat-title-settings" ->
                """{"ok":true,"enabled":$enabled,"models":{"codex":"gpt-6-luna","claude":"claude-haiku-5-5"}}"""
            "agent-catalog" -> CATALOG
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val CATALOG = """{"ok":true,"backends":[""" +
            """{"id":"codex","name":"Codex","models":[{"id":"gpt-6-sol","name":"GPT-6 Sol"},{"id":"gpt-6-luna","name":"GPT-6 Luna"}]},""" +
            """{"id":"claude","name":"Claude","models":[{"id":"claude-opus-5-5","name":"Claude Opus 5.5"},{"id":"claude-haiku-5-5","name":"Claude Haiku 5.5"}]}]}"""
    }
}
