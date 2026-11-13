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
import androidx.compose.ui.semantics.SemanticsActions
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
 * Renders the full-screen new session form with its workspace choices loaded,
 * and drives it as a person would: switching to a new worktree, picking a
 * branch on its own page, and starting the session.
 *
 * With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class NewSessionDialogScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun formDark() = shoot("new-session-form-dark", MinimalThemePreset.DARK)

    @Test fun formLight() = shoot("new-session-form-light", MinimalThemePreset.LIGHT)

    /** A chat joining a session: no workspace to choose, and nothing to wait for once options load. */
    @Test fun chatTabDark() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        NewSessionDialog(
                            projectName = "Reconnect backoff",
                            backends = listOf(com.restartfu.xd.protocol.BackendReply("claude", "Claude", efforts = listOf("low", "medium", "high"))),
                            chatTab = true,
                            onDismiss = {},
                            onCreate = {},
                        )
                    }
                }
            }
        }
        repeat(3) { idle() }
        val screen = activity.window.decorView
        val dialog = dialogViews().last { it !== screen }
        check(shows(dialog, "New chat") && shows(dialog, "Shares this session's workspace") && !shows(dialog, "Workspace")) { "A chat shows its session, not a workspace" }
        check(isEnabled(dialog, "Start chat")) { "Start chat waits for nothing" }
        capture(screen, dialog, "new-chat-dark")
    }

    /** An assistant with several accounts offers them, and a chat is made on the one chosen. */
    @Test fun aNewChatCanBeSetToAnAccount() {
        val accounts = listOf(
            com.restartfu.xd.protocol.AgentAccountReply(id = "default", default = true, label = "dana@example.com", state = "signed-in", inUse = true),
            com.restartfu.xd.protocol.AgentAccountReply(id = "b7c8d9e0f1a2", label = "dana.work@example.com", state = "signed-in"),
        )
        var created: NewSessionRequest? = null
        // Only Codex has two; Claude's lone account is no choice.
        val listed = mapOf("codex" to accounts, "claude" to emptyList())
        fun open(
            draftKey: String,
            offered: Map<String, List<com.restartfu.xd.protocol.AgentAccountReply>>? = listed,
        ): Pair<ComponentActivity, View> {
            val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
            activity.setContent {
                CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                    MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                        Surface(Modifier.fillMaxSize()) {
                            NewSessionDialog(
                                projectName = "Reconnect backoff",
                                draftKey = draftKey,
                                backends = listOf(com.restartfu.xd.protocol.BackendReply("claude", "Claude", efforts = listOf("low", "medium", "high"))),
                                accounts = offered,
                                chatTab = true,
                                onDismiss = {},
                                onCreate = { created = it },
                            )
                        }
                    }
                }
            }
            repeat(3) { idle() }
            return activity to dialogViews().last { it !== activity.window.decorView }
        }

        // The host chooses until an account is picked.
        val (first, form) = open("account-choice/automatic")
        check(shows(form, "Account") && shows(form, "Automatic") && shows(form, "The first account with room left")) { "The host chooses at first" }
        capture(first.window.decorView, form, "new-chat-account-dark")
        click(form, "Start chat")
        check(checkNotNull(created) { "Start chat sent nothing" }.account == null) { created.toString() }
        first.finish()

        // A form left with an account picked comes back with it, and sends it.
        created = null
        first.getSharedPreferences("new-chat-drafts", android.content.Context.MODE_PRIVATE).edit()
            .putString("account-choice/picked", """{"agent":"codex","account":"b7c8d9e0f1a2"}""").commit()
        val (second, picked) = open("account-choice/picked")
        check(shows(picked, "dana.work@example.com") && !shows(picked, "The first account with room left")) { "The form shows the account picked" }
        capture(second.window.decorView, picked, "new-chat-account-chosen-dark")
        click(picked, "Start chat")
        check(checkNotNull(created) { "Start chat sent nothing" }.account == "b7c8d9e0f1a2") { created.toString() }

        // Accounts belong to one assistant: another has none to choose.
        created = null
        click(picked, "Claude")
        check(!shows(picked, "Account")) { "Claude's lone account is no choice" }
        click(picked, "Start chat")
        check(checkNotNull(created).account == null) { created.toString() }
        second.finish()

        // Started before the host has listed the accounts, the account picked
        // is still sent, for the host to check, rather than lost.
        val drafts = second.getSharedPreferences("new-chat-drafts", android.content.Context.MODE_PRIVATE)
        fun started(draftKey: String, offered: Map<String, List<com.restartfu.xd.protocol.AgentAccountReply>>?): String? {
            created = null
            drafts.edit().putString(draftKey, """{"agent":"codex","account":"b7c8d9e0f1a2"}""").commit()
            val (activity, form) = open(draftKey, offered)
            click(form, "Start chat")
            activity.finish()
            return checkNotNull(created) { "Start chat sent nothing" }.account
        }
        check(started("account-choice/unlisted", null) == "b7c8d9e0f1a2") { "Kept until the accounts are listed" }
        // Listed without it, it was removed since, and is not sent.
        check(started("account-choice/removed", mapOf("codex" to accounts.take(1))) == null) { "A removed account is not sent" }
    }

    @Test fun creationDraftSurvivesClosingAndReopening() {
        fun open(): ComponentActivity {
            val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
            activity.setContent {
                MaterialTheme {
                    NewSessionDialog(projectName = "Draft test", draftKey = "draft-reopen/new",
                        onDismiss = {}, onCreate = {})
                }
            }
            idle()
            return activity
        }
        val first = open()
        val dialog = dialogViews().last { it !== first.window.decorView }
        val editor = nodes(dialog).first { it.config.getOrNull(SemanticsActions.SetText) != null }
        editor.config[SemanticsActions.SetText].action!!(androidx.compose.ui.text.AnnotatedString("Keep my draft"))
        idle()
        first.finish()
        val second = open()
        val reopened = dialogViews().last { it !== second.window.decorView }
        assertTrue(nodes(reopened).any { it.config.getOrNull(SemanticsProperties.EditableText)?.text == "Keep my draft" })
        second.finish()
    }

    private fun shoot(name: String, theme: MinimalThemePreset) {
        // A plain activity: the compose test rule waits for an idleness this
        // dialog never reaches under Robolectric.
        var created: NewSessionRequest? = null
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        NewSessionDialog(
                            projectName = "xd",
                            backends = listOf(com.restartfu.xd.protocol.BackendReply("codex", "Codex", efforts = listOf("low", "medium", "high", "xhigh"))),
                            places = { PLACES },
                            onDismiss = {},
                            onCreate = { created = it },
                        )
                    }
                }
            }
        }
        repeat(3) { idle() }
        val screen = activity.window.decorView
        val dialog = dialogViews().last { it !== screen }
        // The project's own checkout is chosen at first, listed under Current worktree.
        check(topOf(dialog, "Current worktree") < topOf(dialog, "master")) { "Current worktree comes before the checkouts" }
        check(topOf(dialog, "Assistant") < topOf(dialog, "Workspace")) { "Assistant comes before Workspace" }
        check(topOf(dialog, "Workspace") < topOf(dialog, "Options")) { "Workspace comes before Options" }
        check(shows(dialog, "Reasoning effort") && shows(dialog, "Access") && shows(dialog, "Mode")) { "Options are all there" }
        check(isEnabled(dialog, "Start session")) { "Start waits for nothing once the checkouts are read" }
        capture(screen, dialog, name)

        // Tapping the segment already chosen keeps the checkout picked under it.
        click(dialog, "feat/usage-page")
        click(dialog, "Current worktree")
        check(nodes(dialog).any { node ->
            node.config.getOrNull(SemanticsProperties.Selected) == true &&
                descendants(node).any { it.config.getOrNull(SemanticsProperties.Text)?.any { text -> text.text == "feat/usage-page" } == true }
        }) { "The chosen checkout stays chosen" }

        click(dialog, "New worktree")
        check(shows(dialog, "Fresh branch") && shows(dialog, "From a branch or pull request") && !shows(dialog, "master")) { "New worktree offers its starts" }
        capture(screen, dialog, "$name-worktree")

        click(dialog, "From a branch or pull request")
        check(shows(dialog, "Pull requests") && shows(dialog, "Branches")) { "The base page lists both" }
        check(topOf(dialog, "#212 Give phone chats one header row") < topOf(dialog, "fix/reconnect-backoff")) { "Pull requests come first" }
        capture(screen, dialog, "$name-base")

        click(dialog, "fix/reconnect-backoff")
        // The chosen branch stays listed whatever the filter hides.
        click(dialog, "Change branch or pull request")
        type(dialog, "docs")
        check(shows(dialog, "docs/protocol") && shows(dialog, "fix/reconnect-backoff")) { "The chosen branch stays listed under a filter" }
        click(dialog, "fix/reconnect-backoff")
        // Back on the form, with the branch chosen and its remote option offered.
        check(shows(dialog, "fix/reconnect-backoff") && shows(dialog, "Start from remote")) { "The form shows the chosen branch" }
        val toggle = nodes(dialog).single { it.config.getOrNull(SemanticsProperties.ToggleableState) != null }
        check(toggle.config[SemanticsProperties.ToggleableState] == androidx.compose.ui.state.ToggleableState.On)
        check(toggle.config[SemanticsActions.OnClick].action?.invoke() == true)
        idle()
        click(dialog, "Start session")
        val request = checkNotNull(created) { "Start session sent nothing" }
        check(request.worktree == com.restartfu.xd.protocol.Ops.NewSessionWorktree.Branch("refs/heads/fix/reconnect-backoff", "a1")) { request.toString() }
        check(!request.startFromRemote && request.agent == com.restartfu.xd.model.DirectAgent.CODEX && !request.plan)
    }

    /**
     * Lets time pass, then draws every window once: under Robolectric layout
     * only catches up when something draws, and a lazy list composes its rows
     * in layout.
     */
    /** Sets the first text field's text without focusing it, which would keep Compose busy. */
    private fun type(root: View, text: String) {
        val field = nodes(root).first { it.config.getOrNull(SemanticsActions.SetText) != null }
        field.config[SemanticsActions.SetText].action!!(androidx.compose.ui.text.AnnotatedString(text))
        repeat(2) { idle() }
    }

    /** A pull request listing that failed, with nothing else to start from, still says why. */
    @Test fun aFailedPullRequestListingIsShown() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent {
            MaterialTheme {
                NewSessionDialog(
                    projectName = "xd",
                    places = { PLACES.copy(canCreateWorktree = false, branches = emptyList(), pullRequests = emptyList(), pullRequestError = "GitHub CLI is not signed in") },
                    onDismiss = {},
                    onCreate = {},
                )
            }
        }
        repeat(3) { idle() }
        val dialog = dialogViews().last { it !== activity.window.decorView }
        check(shows(dialog, "GitHub CLI is not signed in")) { "The pull request error shows on the form" }
        activity.finish()
    }

    private fun idle() {
        repeat(10) { ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS) }
        dialogViews().filter { it.width > 0 && it.height > 0 }.forEach { view ->
            view.draw(Canvas(Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)))
        }
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS)
    }

    private fun nodes(root: View): List<SemanticsNode> =
        composeViews(root).flatMap { descendants(it.semanticsOwner.unmergedRootSemanticsNode) }

    private fun node(root: View, text: String): SemanticsNode = nodes(root).firstOrNull { node ->
        node.config.getOrNull(SemanticsProperties.Text)?.any { it.text == text } == true
    } ?: error("Nothing shows \"$text\"")

    private fun shows(root: View, text: String) = nodes(root).any { node ->
        node.config.getOrNull(SemanticsProperties.Text)?.any { it.text == text } == true
    }

    /** The nearest node at or above [text] that can be clicked. */
    private fun clickable(root: View, text: String): SemanticsNode {
        var found: SemanticsNode? = node(root, text)
        while (found != null && found.config.getOrNull(SemanticsActions.OnClick) == null) found = found.parent
        return checkNotNull(found) { "\"$text\" cannot be clicked" }
    }

    private fun isEnabled(root: View, text: String) =
        !clickable(root, text).config.contains(SemanticsProperties.Disabled)

    private fun click(root: View, text: String) {
        check(clickable(root, text).config[SemanticsActions.OnClick].action?.invoke() == true) { "Clicking \"$text\" did nothing" }
        repeat(2) { idle() }
    }

    private fun capture(screen: View, dialog: View, name: String) {
        val dir = out ?: return
        dir.mkdirs()
        val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        screen.draw(canvas)
        canvas.drawColor(Color.argb(82, 0, 0, 0))
        canvas.translate((screen.width - dialog.width) / 2f, (screen.height - dialog.height) / 2f)
        dialog.draw(canvas)
        File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
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
