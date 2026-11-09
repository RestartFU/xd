package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import android.graphics.Canvas
import android.graphics.Color
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.ui.platform.ViewRootForTest
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import java.util.concurrent.TimeUnit
import org.robolectric.Robolectric
import org.robolectric.shadows.ShadowLooper
import com.restartfu.xd.protocol.BranchSourceReply
import com.restartfu.xd.protocol.PullRequestSourceReply
import com.restartfu.xd.protocol.WorktreeReply
import java.io.File
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Renders the new session dialog with its workspace choices loaded.
 *
 * It checks that New worktree is listed before the project's checkouts, as
 * the desktop's Start from list orders them. With `-Pxd.shots=<dir>` it also
 * writes a PNG capture there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class NewSessionDialogScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun newWorktreeComesFirstDark() = shoot("new-session-dark", MinimalThemePreset.DARK)

    @Test fun newWorktreeComesFirstLight() = shoot("new-session-light", MinimalThemePreset.LIGHT)

    private fun shoot(name: String, theme: MinimalThemePreset) {
        // A plain activity: the compose test rule waits for an idleness this
        // dialog never reaches under Robolectric.
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        NewSessionDialog(
                            projectName = "xd",
                            places = { PLACES },
                            onDismiss = {},
                            onCreate = { _, _, _, _, _, _ -> },
                        )
                    }
                }
            }
        }
        repeat(30) { ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS) }
        val screen = activity.window.decorView
        val dialog = dialogViews().last { it !== screen }
        val newWorktree = topOf(dialog, "New worktree")
        val checkout = topOf(dialog, "master")
        val branch = topOf(dialog, "fix/reconnect-backoff")
        assertTrue("New worktree is listed before the checkouts", newWorktree < checkout)
        assertTrue("Checkouts are listed before branches", checkout < branch)
        out?.let { dir ->
            dir.mkdirs()
            val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            screen.draw(canvas)
            canvas.drawColor(Color.argb(82, 0, 0, 0))
            canvas.translate((screen.width - dialog.width) / 2f, (screen.height - dialog.height) / 2f)
            dialog.draw(canvas)
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    /** The unclipped top edge of the first node in [root]'s Compose tree showing [text]. */
    private fun topOf(root: View, text: String): Float {
        val nodes = composeViews(root).flatMap { descendants(it.semanticsOwner.unmergedRootSemanticsNode) }
        val node = nodes.firstOrNull { node ->
            node.config.getOrNull(SemanticsProperties.Text)?.any { it.text == text } == true
        } ?: error("Nothing shows \"$text\"")
        return node.positionInRoot.y
    }

    private fun descendants(node: SemanticsNode): List<SemanticsNode> =
        listOf(node) + node.children.flatMap(::descendants)

    private fun composeViews(view: View): List<ViewRootForTest> = when (view) {
        is ViewRootForTest -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { composeViews(view.getChildAt(it)) }
        else -> emptyList()
    }

    @Suppress("UNCHECKED_CAST")
    private fun dialogViews(): List<android.view.View> {
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        return (field.get(instance) as List<android.view.View>).toList()
    }

    private companion object {
        val PLACES = NewSessionPlaces(
            worktrees = listOf(
                WorktreeReply(path = "/home/dev/xd", branch = "master", detached = false, main = true, current = true),
                WorktreeReply(path = "/home/dev/.xd/worktrees/xd/usage-page", branch = "feat/usage-page", detached = false, main = false, current = false),
            ),
            canCreateWorktree = true,
            branches = listOf(
                BranchSourceReply("fix/reconnect-backoff", "refs/heads/fix/reconnect-backoff", "a1"),
                BranchSourceReply("docs/protocol", "refs/heads/docs/protocol", "b2"),
            ),
            pullRequests = listOf(PullRequestSourceReply(212, "Give phone chats one header row", "c3")),
            pullRequestError = null,
        )
    }
}
