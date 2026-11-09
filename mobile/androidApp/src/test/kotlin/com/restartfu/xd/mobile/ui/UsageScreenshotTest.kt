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
        val client = XdClient({ error("No network in fixture") }, NoCredentials, scope)
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
            accounts = listOf(UsageAccountReply(provider = "codex", plan = "Pro")))
        settle()
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
        scope.cancel()
    }
    private fun settle() { repeat(20) { compose.mainClock.advanceTimeBy(100); Thread.sleep(10) }; org.robolectric.shadows.ShadowLooper.idleMainLooper() }
    private fun capture(name: String) {
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bitmap))
        System.getProperty("xd.shots")?.let { path ->
            File(path).mkdirs()
            File(path, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }
    private object NoCredentials : CredentialStore {
        override suspend fun load(): StoredCredentials? = null
        override suspend fun save(credentials: StoredCredentials) = Unit
        override suspend fun clear() = Unit
    }
}
