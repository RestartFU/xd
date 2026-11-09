package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.core.app.ApplicationProvider
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.UsageViewModel
import com.restartfu.xd.mobile.UsageState
import com.restartfu.xd.protocol.*
import kotlinx.coroutines.flow.MutableStateFlow
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.model.ChatSummary
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

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UsageScreenshotTest {
    @get:Rule val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()
    @Test fun usage() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ PriceSocket() }, NoCredentials, scope)
        val model = UsageViewModel(client, MobileSettings(compose.activity))
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { UsageContent(model) }
                }
            }
        }
        settle()
        val field = UsageViewModel::class.java.getDeclaredField("_state").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val state = field.get(model) as MutableStateFlow<UsageState>
        state.value = UsageState(days = 30, summary = UsageSummaryReply(
            windowLabel = "Past 30 days",
            periods = (1..7).map { UsagePeriodReply("day$it", "Nov $it") },
            buckets = (1..7).flatMap { day -> listOf(
                UsageBucketReply("day$day", provider = "codex", model = "gpt-5", costUsd = day * 0.4, totals = UsageTotalsReply(uncachedInput = day * 10000L, output = day * 2000L)),
                UsageBucketReply("day$day", provider = "claude", model = "claude-sonnet", costUsd = day * 0.2, totals = UsageTotalsReply(uncachedInput = day * 5000L, output = day * 1000L)),
            ) }, sessions = mapOf("codex" to 4, "claude" to 2)),
            accounts = listOf(
                UsageAccountReply(provider = "codex", accountKey = "fixture-codex", plan = "Pro", windows = listOf(
                    UsageWindowReply("primary", "session", "Session", 38.0, resetsAtLabel = "today at 18:00"),
                    UsageWindowReply("secondary", "weekly", "Weekly", 92.0, resetsAtLabel = "Friday at 09:00"),
                ), resetCredits = ResetCreditsReply(2)),
                UsageAccountReply(provider = "claude", plan = "Max", windows = listOf(
                    UsageWindowReply("primary", "session", "Session", 24.0, resetsAtLabel = "today at 20:00"),
                    UsageWindowReply("secondary", "weekly", "Weekly", 61.0, resetsAtLabel = "Saturday at 09:00"),
                )),
            ))
        settle()
        compose.onNodeWithText("62% left").assertIsDisplayed()
        compose.onNodeWithText("8% left").assertIsDisplayed()
        // The accessible progress must describe remaining quota too, not just its label.
        compose.onNode(androidx.compose.ui.test.SemanticsMatcher.expectValue(
            androidx.compose.ui.semantics.SemanticsProperties.ProgressBarRangeInfo,
            androidx.compose.ui.semantics.ProgressBarRangeInfo(0.62f, 0f..1f),
        )).assertIsDisplayed()
        compose.onNode(androidx.compose.ui.test.SemanticsMatcher.expectValue(
            androidx.compose.ui.semantics.SemanticsProperties.ProgressBarRangeInfo,
            androidx.compose.ui.semantics.ProgressBarRangeInfo(0.08f, 0f..1f),
        )).assertIsDisplayed()
        capture("overview")
        compose.onNodeWithText("Cost").performClick()
        settle()
        compose.onNodeWithText("$16.80").assertIsDisplayed()
        capture("cost")
        compose.onNodeWithText("Tokens").performClick()
        settle()
        capture("tokens")
        compose.onNodeWithText("Claude").performClick()
        settle()
        compose.onAllNodes(androidx.compose.ui.test.hasText("168.0K"))[0].assertIsDisplayed()
        capture("provider")
        compose.onNodeWithText("claude-sonnet").performClick()
        settle()
        compose.onNodeWithText("Back to overview").assertIsDisplayed()
        capture("model")
        compose.onNodeWithText("Day").performClick()
        settle()
        capture("day")
        compose.runOnUiThread { compose.activity.onBackPressedDispatcher.onBackPressed() }
        settle()
        compose.onNodeWithText("Cost").performClick()
        settle()
        compose.onNodeWithText("$5.60").assertIsDisplayed()
        // Complete the Limits evidence with populated windows and reset confirmation.
        model.showProvider("All providers")
        model.showMetric("Limits")
        settle()
        compose.onNodeWithText("Use a credit").performScrollTo().performClick()
        settle()
        capture("reset-confirmation")
        compose.onNodeWithText("Cancel").performClick()
        settle()
        val populated = state.value
        state.value = populated.copy(accounts = listOf(UsageAccountReply(provider = "claude", unavailable = UsageUnavailableReply("probeFailed", "Could not read Claude limits. Try refreshing."))))
        settle()
        capture("limits-unavailable")
        state.value = populated.copy(accounts = emptyList())
        settle()
        capture("limits-empty")
        state.value = populated.copy(accounts = null, limitsError = "The host could not read limits.")
        settle()
        capture("limits-error")
        state.value = populated.copy(accounts = null, limitsLoading = true)
        settle()
        capture("limits-loading")
        state.value = populated
        runBlocking { client.connect(SshConnection("host", 22, "dev", SshAuthentication.Password("x"), SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:test"))) }
        settle()
        check(runBlocking { model.prices() }.prices.containsKey("custom-model"))
        val edit = compose.onNodeWithText("Edit").performScrollTo().fetchSemanticsNode()
        // A populated editor has blinking text cursors; do not wait for idle after opening it.
        compose.runOnUiThread { edit.config[androidx.compose.ui.semantics.SemanticsActions.OnClick].action?.invoke() }
        settle()
        capture("prices")
        scope.cancel()
    }
    private fun settle() { repeat(20) { compose.mainClock.advanceTimeBy(100); Thread.sleep(10) }; org.robolectric.shadows.ShadowLooper.idleMainLooper() }
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
        System.getProperty("xd.shots")?.let { path ->
            File(path).mkdirs()
            File(path, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }
    /** Only price reads are used; no reset operation can reach a real host. */
    private class PriceSocket : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }
        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = if (op == "usage-prices")
                    """{"ok":true,"aliases_optional":true,"prices":{"custom-model":{"input":2.0,"output":8.0,"cache_read":0.2,"cache_write":2.5}}}"""
                    else if (op == "tree") """{"ok":true,"folders":[],"chats":[]}"""
                    else """{"ok":true}"""
                val reply = body.dropLast(1) + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { listener?.onBytes((reply + "\n").encodeToByteArray()) }.start()
            }
        }
        override fun close() = Unit
    }
    private object NoCredentials : CredentialStore {
        override suspend fun load(): StoredCredentials? = null
        override suspend fun save(credentials: StoredCredentials) = Unit
        override suspend fun clear() = Unit
    }
}
