package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.performTextReplacement
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.File
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the error-state, Usage-width and code-colour work
 * changed, from the app against [AuditHost]. With `-Pxd.shots=<dir>` it
 * writes `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before that work, so the same file run on the commit before it gives the
 * matching "before" captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxRefineShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A pairing code that is not one: the form says why. */
    @Test fun pairingError() = shootPairing("android-pairing-error-dark", NoAnswerClient().client, code = "ABCD-EFGH")

    /** A good code sent to a host that has not answered yet. */
    @Test fun pairingConnecting() {
        val silent = NoAnswerClient()
        shootPairing("android-pairing-connecting-dark", silent.client, code = "xd2:ABCD:" + "A".repeat(43))
        silent.close()
    }

    /** The projects were read once; reading them again fails. */
    @Test fun projectsError() {
        val failing = AtomicBoolean(false)
        shoot("android-projects-error-dark", MinimalThemePreset.DARK, override = { op, _ ->
            if (op == "tree" && failing.get()) """{"ok":false,"error":"The host could not read its projects."}""" else null
        }) {
            failing.set(true)
            host.push("""{"event":"tree"}""")
            await { model.client.tree.value.error != null }
            repeat(2) { settle() }
        }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletUsage() = shoot("android-tablet-usage-dark", MinimalThemePreset.DARK) {
        click("Usage")
        click("Cost")
        await { request("usage-summary") != null && request("usage-limits") != null }
        host.push(
            """{"event":"usage-summary","generation":${generation("usage-summary")},"summary":$SUMMARY}""",
        )
        host.push("""{"event":"usage-limits","generation":${generation("usage-limits")},"accounts":[]}""")
        await { shows("API estimate") }
        repeat(3) { settle() }
    }

    /** A reply with a code block in every token colour, on Light. */
    @Test fun chatCodeLight() = shoot("android-chat-code-light", MinimalThemePreset.LIGHT, override = { op, request ->
        if (op == "messages" && (request["chat"] as? JsonPrimitive)?.content == "c1") CODE_MESSAGES else null
    }) {
        click("Reconnect backoff")
        await { shows("Backoff now starts at 2 s.") }
        repeat(4) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** The shared terminal could not be opened. */
    @Test fun terminalError() = shoot("android-terminal-error-dark", MinimalThemePreset.DARK, override = { op, _ ->
        if (op == "terminal-list") """{"ok":false,"error":"The host could not start a shell."}""" else null
    }) {
        click("Terminal")
        await { shows("The host could not start a shell.") }
        repeat(2) { settle() }
    }

    private lateinit var host: AuditHost
    private lateinit var model: MainViewModel

    private fun request(op: String): JsonObject? = host.sent.lastOrNull { (it["op"] as? JsonPrimitive)?.content == op }

    private fun generation(op: String): String = (request(op)?.get("generation") as JsonPrimitive).content

    private fun ProjectsScreenshotTest.Screen.await(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(condition()) { "Timed out; showing ${labels()}" }
    }

    private fun shoot(
        name: String,
        theme: MinimalThemePreset,
        override: (String?, JsonObject) -> String? = { _, _ -> null },
        pushSignals: Boolean = true,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        host = AuditHost(override = override).connect()
        model = MainViewModel(activity.application, host.client)
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            XdRoot(theme) { MinimalMobileApp(model, link, settings) }
        }
        val screen = ProjectsScreenshotTest.Screen(compose, activity)
        try {
            screen.await { model.client.tree.value.folders.isNotEmpty() }
            if (pushSignals) {
                host.pushSignals()
                screen.await {
                    "c8" in model.client.waitingChats.value && "c9" in model.client.unreadChats.value
                }
            }
            screen.settle()
            screen.steps()
        } catch (failure: Throwable) {
            capture(screen, "$name-failed")
            throw failure
        }
        capture(screen, name)
        host.close()
    }

    /** The pairing form with an address and [code] filled in, after Pair and connect. */
    private fun shootPairing(name: String, client: XdClient, code: String) {
        val activity = compose.activity
        val pairing = MainViewModel(activity.application, client)
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(MinimalThemePreset.DARK) { ConnectScreen(pairing) } }
        val screen = ProjectsScreenshotTest.Screen(compose, activity)
        screen.settle()
        compose.onNode(hasTestTag(PAIRING_ADDRESS_TAG)).performTextReplacement("192.168.1.20")
        compose.onNode(hasTestTag(PAIRING_CODE_TAG)).performTextReplacement(code)
        screen.settle()
        screen.click("Pair and connect")
        repeat(3) {
            ShadowLooper.idleMainLooper()
            screen.settle()
        }
        capture(screen, name)
    }

    private fun capture(screen: ProjectsScreenshotTest.Screen, name: String) {
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    /** A client with no pairing whose host never answers, so a connection stays under way. */
    private class NoAnswerClient {
        private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ Silent() }, Empty(), scope)

        fun close() = scope.cancel()

        private class Silent : PlatformSocket {
            override fun connect(connection: SshConnection, listener: PlatformSocketListener) = Unit
            override fun send(bytes: ByteArray) = Unit
            override fun close() = Unit
        }

        private class Empty : CredentialStore {
            override suspend fun load(): StoredCredentials? = null
            override suspend fun save(credentials: StoredCredentials) = Unit
            override suspend fun clear() = Unit
        }
    }

    private companion object {
        const val TABLET = "w1280dp-h800dp-320dpi"

        private fun messages(vararg rows: Pair<String, String>): String =
            """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                rows.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"

        val CODE_MESSAGES = messages(
            "user" to "Show me the new delay curve.",
            "assistant" to "Here is the new `Backoff`, with a cap:\n\n" +
                "```kotlin\n" +
                "// Waits longer after each failure, up to two minutes.\n" +
                "@JvmInline\n" +
                "value class Delay(val millis: Long)\n\n" +
                "fun next(attempt: Int): Delay = when (attempt) {\n" +
                "    0 -> Delay(2_000L)\n" +
                "    else -> Delay(minOf(2_000L shl attempt, 120_000L))\n" +
                "}\n\n" +
                "val label = \"retrying in ${'$'}{next(3).millis / 1000} s\"\n" +
                "```\n\n" +
                "Backoff now starts at 2 s.",
        )

        val SUMMARY: String = run {
            val days = (1..7).map { "2026-11-0$it" }
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
    }
}
