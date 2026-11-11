package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextReplacement
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.InteractionAnswers
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.net.ConnectResult
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
import kotlinx.serialization.json.JsonPrimitive
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
 * What waits on the reader, docked in the composer: a tagged question, the
 * agent's own questions, approvals, and a quota stop, against a scripted
 * host. With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class AskSectionScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun taggedQuestionDark() = shoot("ask-tagged-question-dark", Scene(TAGGED)) {
        check(shows("How should we roll out this release?"))
        check(shows("Start with a small pilot") && shows("Release to everyone"))
        // No second status: the header, tab and sidebar already say it waits.
        check(!shows("Waiting on you"))
        check(shows("Or type your own answer…"))
    }

    @Test fun taggedQuestionLight() = shoot("ask-tagged-question-light", Scene(TAGGED), MinimalThemePreset.LIGHT) {
        check(shows("How should we roll out this release?"))
    }

    @Test fun agentQuestionDark() = shoot("ask-agent-question-dark", Scene(DRY_RUN, interactions = QUESTIONS)) {
        check(shows("How should we roll out this release?") && shows("1 / 2"))
        check(!shows("Submit answer") && !shows("Waiting on you"))
    }

    @Test fun approvalDark() = shoot("ask-approval-dark", Scene(DRY_RUN, interactions = APPROVALS)) {
        check(shows("Run this command?") && shows("1 / 2"))
        check(shows("cargo publish --dry-run --allow-dirty"))
        check(shows("Allow once") && shows("Allow for session") && shows("Deny"))
        check(!shows("Waiting on you"))
    }

    @Test fun approvalLight() = shoot("ask-approval-light", Scene(DRY_RUN, interactions = APPROVALS), MinimalThemePreset.LIGHT) {
        check(shows("Run this command?"))
    }

    @Test fun quotaDark() = shoot("ask-quota-dark", Scene(DRY_RUN, quotaReached = true)) {
        check(shows("This chat reached its quota") && !shows("Waiting on you"))
        check(shows("Continue without a limit"))
        check(shows("Messages you send wait until it goes on…"))
    }

    @Config(qualifiers = "w900dp-h700dp-240dpi")
    @Test fun approvalTabletDark() = shoot("ask-approval-tablet-dark", Scene(DRY_RUN, interactions = APPROVALS), compact = false) {
        check(shows("Run this command?"))
    }

    @Config(qualifiers = "w900dp-h700dp-240dpi")
    @Test fun taggedQuestionTabletDark() = shoot("ask-tagged-question-tablet-dark", Scene(TAGGED), compact = false) {
        check(shows("Or type your own answer…"))
    }

    /** Tapping an option sends it as its own message and leaves the draft alone. */
    @Test fun anOptionAnswersATaggedQuestion() = shoot(null, Scene(TAGGED, draft = "Also bump the version.")) {
        click("Release to everyone")
        val sent = await("send")
        check(sent["text"]?.jsonPrimitive?.content == "Release to everyone") { "Sent $sent" }
        check(chat.draft.value == "Also bump the version.") { "The draft became ${chat.draft.value}" }
    }

    /** A typed answer to a tagged question goes from the composer, which empties after. */
    @Test fun aTypedAnswerGoesFromTheComposer() = shoot(null, Scene(TAGGED)) {
        compose.onNode(hasSetTextAction()).performTextReplacement("Staff first, then everyone")
        settle()
        clickDescribed("Send")
        val sent = await("send")
        check(sent["text"]?.jsonPrimitive?.content == "Staff first, then everyone") { "Sent $sent" }
        awaitEmptyDraft()
    }

    /**
     * The agent's questions come one at a time. A draft written before they
     * appeared stays, and Send makes it the answer to the one showing.
     */
    @Test fun theAgentsQuestionsTakeAnOptionThenTheDraft() = shoot(
        null,
        Scene(DRY_RUN, interactions = QUESTIONS, draft = "Friday morning"),
    ) {
        check(chat.draft.value == "Friday morning")
        click("Start with a small pilot")
        awaitShown("When should it go out?")
        check(shows("2 / 2") && !shows("How should we roll out this release?"))
        check(chat.draft.value == "Friday morning") { "The draft became ${chat.draft.value}" }
        clickDescribed("Send answer")
        val request = await("respond")
        check(request["request_id"]?.jsonPrimitive?.content == "ask-1") { "Answered $request" }
        val answers = request["response"]!!.jsonObject["answers"]!!.jsonObject
        fun answer(id: String) = answers[id]?.jsonObject?.get("answers")?.jsonArray?.single()?.jsonPrimitive?.content
        check(answer("rollout") == "Start with a small pilot" && answer("when") == "Friday morning") { "Answered $answers" }
        awaitEmptyDraft()
    }

    /**
     * A typed answer is sent like a message: the composer empties at once.
     * While it is on its way Send is a busy send, not the turn's stop;
     * refused, the words come back to the empty composer and the question
     * asks again, and sending again works.
     */
    @Test fun aRefusedAnswerGivesItsWordsBack() = shoot(
        null,
        Scene(DRY_RUN, interactions = QUESTIONS, draft = "Friday morning", holdAnswers = true, refuseFirstAnswer = true),
    ) {
        click("Start with a small pilot")
        awaitShown("When should it go out?")
        clickDescribed("Send answer")
        await("respond")
        check(chat.submittingAnswers.value == setOf("ask-1")) { "Submitting ${chat.submittingAnswers.value}" }
        check(chat.draft.value.isEmpty()) { "The draft is still ${chat.draft.value}" }
        settle()
        compose.onNode(hasContentDescription("Sending answer")).assertIsNotEnabled()
        check(!describes("Stop") && !describes("Send") && !describes("Send answer"))

        socket.releaseAnswers()
        awaitAnswersSettled()
        awaitDraft("Friday morning")
        settle()
        check(shows("When should it go out?") && shows("2 / 2"))
        compose.onNode(hasContentDescription("Send answer")).assertIsEnabled()

        clickDescribed("Send answer")
        awaitEmptyDraft()
        awaitAnswersSettled()
        check(socket.sent("respond").size == 2) { "Answered ${socket.sent("respond").size} times" }
        check(chat.draft.value.isEmpty()) { "The draft became ${chat.draft.value}" }
    }

    /** Taken, an answer leaves the composer alone: what was typed since stays. */
    @Test fun aTakenAnswerLeavesWhatWasTypedSince() = shoot(
        null,
        Scene(DRY_RUN, interactions = QUESTIONS, draft = "Friday morning", holdAnswers = true),
    ) {
        click("Start with a small pilot")
        awaitShown("When should it go out?")
        clickDescribed("Send answer")
        await("respond")
        check(chat.draft.value.isEmpty()) { "The draft is still ${chat.draft.value}" }
        compose.onNode(hasSetTextAction()).performTextReplacement("Also tag the release")
        settle()
        socket.releaseAnswers()
        awaitAnswersSettled()
        settle()
        check(chat.draft.value == "Also tag the release") { "The draft became ${chat.draft.value}" }
    }

    /** Another request that comes while one is on its way is an ordinary ask, its composer empty. */
    @Test fun aNewRequestWhileOneIsOnItsWayStartsEmpty() = shoot(
        null,
        Scene(DRY_RUN, interactions = QUESTIONS, draft = "Friday morning", holdAnswers = true),
    ) {
        click("Start with a small pilot")
        awaitShown("When should it go out?")
        clickDescribed("Send answer")
        await("respond")
        socket.push("""{"event":"interaction-resolved","chat":"chat","request_id":"ask-1","interaction_revision":1}""")
        socket.push(
            """{"event":"interaction","chat":"chat","request_id":"ask-2","method":"item/tool/requestUserInput",""" +
                """"params":{"questions":[{"id":"notes","question":"Anything for the release notes?","options":[]}]},""" +
                """"interaction_revision":2}""",
        )
        awaitShown("Anything for the release notes?")
        check(chat.draft.value.isEmpty()) { "The draft became ${chat.draft.value}" }
        check(shows("Or type your own answer…"))
        check(!describes("Send answer") && !describes("Sending answer"))

        socket.releaseAnswers()
        awaitAnswersSettled()
        settle()
        check(chat.draft.value.isEmpty()) { "The draft became ${chat.draft.value}" }
        check(shows("Anything for the release notes?"))
    }

    /**
     * Refused while the reader is away from the chat, the words wait, and
     * come back when it is next opened with an empty composer; the answers
     * given before the last one outlive leaving it.
     */
    @Test fun aRefusalWhileAwayComesBackOnReturn() {
        socket = ScriptedSocket(Scene(DRY_RUN, interactions = QUESTIONS, holdAnswers = true, refuseFirstAnswer = true))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        try {
            val store = ViewModelStore()
            val first = ViewModelProvider(store, ChatViewModel.Factory(client, "chat"))[ChatViewModel::class.java]
            awaitThat { first.state.value.interactions.isNotEmpty() }
            val request = first.state.value.interactions.single()
            first.answerQuestion(request, "rollout", "Start with a small pilot")
            first.updateDraft("Friday morning")
            first.answerWithDraft(request, "when")
            check(first.draft.value.isEmpty()) { "The draft is still ${first.draft.value}" }
            await("respond")

            // The reader leaves the chat, and the host refuses meanwhile.
            store.clear()
            socket.releaseAnswers()
            val answers = InteractionAnswers.of(client)
            awaitThat { answers.submittingNow("chat").isEmpty() }

            val second = ViewModelProvider(ViewModelStore(), ChatViewModel.Factory(client, "chat"))[ChatViewModel::class.java]
            awaitThat { second.draft.value == "Friday morning" }
            check(second.interactionAnswers.value["ask-1"] == mapOf("rollout" to "Start with a small pilot")) {
                "Kept ${second.interactionAnswers.value}"
            }
        } finally {
            scope.cancel()
        }
    }

    /**
     * Words waiting for an empty composer never replace the host's draft:
     * back in the chat, nothing is restored until the chat's own reply says
     * what its draft is, and with a draft there the words keep waiting.
     */
    @Test fun waitingWordsNeverReplaceTheHostsDraft() {
        socket = ScriptedSocket(Scene(DRY_RUN, interactions = QUESTIONS, holdAnswers = true, refuseFirstAnswer = true))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        try {
            val store = ViewModelStore()
            val first = ViewModelProvider(store, ChatViewModel.Factory(client, "chat"))[ChatViewModel::class.java]
            awaitThat { first.state.value.interactions.isNotEmpty() }
            val request = first.state.value.interactions.single()
            first.answerQuestion(request, "rollout", "Start with a small pilot")
            first.updateDraft("Friday morning")
            first.answerWithDraft(request, "when")
            await("respond")
            store.clear()
            // The chat's own handle goes once its last draft is saved.
            awaitThat { socket.sent("set-draft").isNotEmpty() }
            repeat(10) {
                ShadowLooper.idleMainLooper()
                Thread.sleep(20)
            }
            socket.releaseAnswers()
            val answers = InteractionAnswers.of(client)
            awaitThat { answers.submittingNow("chat").isEmpty() && answers.givenBackNow("chat") != null }

            // Meanwhile another device left a draft, and the chat's reply is slow.
            socket.draft = "Ship the notes first"
            socket.holdChat()
            val chats = socket.sent("chat").size
            val second = ViewModelProvider(ViewModelStore(), ChatViewModel.Factory(client, "chat"))[ChatViewModel::class.java]
            awaitThat { socket.sent("chat").size > chats }
            repeat(10) {
                ShadowLooper.idleMainLooper()
                Thread.sleep(20)
            }
            check(!second.state.value.draftLoaded)
            check(second.draft.value.isEmpty()) { "Restored ${second.draft.value} before the host's draft" }
            check(answers.givenBackNow("chat") == "Friday morning")

            socket.releaseChat()
            awaitThat { second.state.value.draftLoaded }
            repeat(10) {
                ShadowLooper.idleMainLooper()
                Thread.sleep(20)
            }
            check(second.draft.value == "Ship the notes first") { "The draft became ${second.draft.value}" }
            check(answers.givenBackNow("chat") == "Friday morning") { "Waiting ${answers.givenBackNow("chat")}" }
            check(socket.sent("set-draft").none { it["text"]?.jsonPrimitive?.content == "Friday morning" }) {
                "Saved the words over the host's draft"
            }
        } finally {
            scope.cancel()
        }
    }

    /**
     * One client is reused for another host, whose chat and request ids may
     * be the same: answers kept on one host never show on the other, and are
     * there again on going back.
     */
    @Test fun answersStayWithTheirHost() {
        socket = ScriptedSocket(Scene(DRY_RUN, interactions = QUESTIONS))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        try {
            val hostA = client.host.value
            check(hostA != null)
            fun open(): Pair<ViewModelStore, ChatViewModel> {
                val store = ViewModelStore()
                val chat = ViewModelProvider(store, ChatViewModel.Factory(client, "chat"))[ChatViewModel::class.java]
                awaitThat { chat.state.value.interactions.isNotEmpty() }
                return store to chat
            }
            val (onA, chatA) = open()
            chatA.answerQuestion(chatA.state.value.interactions.single(), "rollout", "Start with a small pilot")
            awaitThat { chatA.interactionAnswers.value["ask-1"] == mapOf("rollout" to "Start with a small pilot") }
            onA.clear()

            check(runBlocking { client.connect(otherHost) } is ConnectResult.Success)
            check(client.host.value != hostA) { "Still on ${client.host.value}" }
            val (onB, chatB) = open()
            repeat(10) {
                ShadowLooper.idleMainLooper()
                Thread.sleep(20)
            }
            check(chatB.interactionAnswers.value.isEmpty()) { "Host A's answers showed on host B: ${chatB.interactionAnswers.value}" }
            check(chatB.state.value.interactions.single().requestId == "ask-1")
            onB.clear()

            check(runBlocking { client.connect(connection) } is ConnectResult.Success)
            check(client.host.value == hostA)
            val (_, back) = open()
            awaitThat { back.interactionAnswers.value["ask-1"] == mapOf("rollout" to "Start with a small pilot") }
        } finally {
            scope.cancel()
        }
    }

    @Test fun allowOnceApproves() = shoot(null, Scene(DRY_RUN, interactions = APPROVALS)) {
        click("Allow once")
        val request = await("respond")
        check(request["request_id"]?.jsonPrimitive?.content == "approve-1") { "Answered $request" }
        check(request["response"]?.jsonObject?.get("decision")?.jsonPrimitive?.content == "accept") { "Answered $request" }
    }

    private lateinit var socket: ScriptedSocket
    private lateinit var chat: ChatViewModel

    private fun shoot(
        name: String?,
        scene: Scene,
        theme: MinimalThemePreset = MinimalThemePreset.DARK,
        compact: Boolean = true,
        steps: () -> Unit,
    ) {
        socket = ScriptedSocket(scene)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        chat = ChatViewModel(client, "chat")
        val chats = listOf(
            ChatSummary("chat", "folder", "Ship the 2.4 release", "claude", false, sessionId = "s"),
            ChatSummary("chat2", "folder", "Release notes", "codex", false, sessionId = "s"),
        )
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = "release-2-4",
                            projectName = "xd",
                            sessionChats = if (compact) SessionChats(chats, "chat", {}, {}, {}, {}, {}) else null,
                        )
                    }
                }
            }
        }
        repeat(60) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
        }
        try {
            ShadowLooper.idleMainLooper()
            if (name != null) {
                val view = compose.activity.window.decorView
                val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
                view.draw(Canvas(bitmap))
                out?.let { dir ->
                    dir.mkdirs()
                    File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
                }
            }
            steps()
        } finally {
            scope.cancel()
        }
    }

    private fun settle() = repeat(10) {
        Thread.sleep(15)
        compose.mainClock.advanceTimeBy(50)
    }

    private fun shows(text: String) =
        compose.onAllNodes(hasText(text, substring = true), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun awaitThat(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 10_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            Thread.sleep(20)
        }
        check(condition()) { "Gave up waiting" }
    }

    private fun awaitAnswersSettled() {
        val deadline = System.currentTimeMillis() + 10_000
        while (chat.submittingAnswers.value.isNotEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(chat.submittingAnswers.value.isEmpty()) { "Still submitting ${chat.submittingAnswers.value}" }
    }

    private fun awaitDraft(text: String) {
        val deadline = System.currentTimeMillis() + 10_000
        while (chat.draft.value != text && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(chat.draft.value == text) { "The draft is ${chat.draft.value}" }
    }

    /** The view model's coroutines resume on the main looper, which only runs when idled. */
    private fun awaitEmptyDraft() {
        val deadline = System.currentTimeMillis() + 5_000
        while (chat.draft.value.isNotEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(chat.draft.value.isEmpty()) { "The draft is still ${chat.draft.value}" }
    }

    private fun awaitShown(text: String) {
        val deadline = System.currentTimeMillis() + 10_000
        while (!shows(text) && System.currentTimeMillis() < deadline) settle()
    }

    private fun click(text: String) {
        compose.onAllNodes(hasText(text)).onFirst().performClick()
        settle()
    }

    private fun describes(description: String) =
        compose.onAllNodes(hasContentDescription(description)).fetchSemanticsNodes().isNotEmpty()

    private fun clickDescribed(description: String) {
        compose.onAllNodes(hasContentDescription(description)).onFirst().performClick()
        settle()
    }

    /** The first [op] request the app sent, waiting for it. */
    private fun await(op: String): JsonObject {
        val deadline = System.currentTimeMillis() + 10_000
        while (socket.sent(op).isEmpty() && System.currentTimeMillis() < deadline) settle()
        return socket.sent(op).firstOrNull() ?: error("The app never sent $op")
    }

    private data class Scene(
        val messages: List<Pair<String, String>>,
        val interactions: String = "",
        val quotaReached: Boolean = false,
        val draft: String = "",
        /** Answers wait for [ScriptedSocket.releaseAnswers]. */
        val holdAnswers: Boolean = false,
        /** The host refuses the first answer. */
        val refuseFirstAnswer: Boolean = false,
    )

    private object NoCredentials : CredentialStore {
        override suspend fun load(): StoredCredentials? = null
        override suspend fun save(credentials: StoredCredentials) = Unit
        override suspend fun clear() = Unit
    }

    private val connection = SshConnection(
        "host", 22, "dev", SshAuthentication.Password("x"),
        SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:test"),
    )

    private val otherHost = SshConnection(
        "other", 22, "dev", SshAuthentication.Password("x"),
        SshHostKey("ssh-ed25519", byteArrayOf(2), "SHA256:other"),
    )

    /** Answers each request from a fixed script, echoing its request id, and keeps what was asked. */
    private class ScriptedSocket(private val scene: Scene) : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        private val requests = CopyOnWriteArrayList<JsonObject>()

        private val answersHeld = java.util.concurrent.CountDownLatch(1)

        fun sent(op: String): List<JsonObject> = requests.filter { it["op"]?.jsonPrimitive?.content == op }

        fun releaseAnswers() = answersHeld.countDown()

        /** The chat's draft as the host has it now. */
        @Volatile var draft: String = scene.draft

        @Volatile private var chatHeld: java.util.concurrent.CountDownLatch? = null

        /** Holds chat replies from now until [releaseChat]. */
        fun holdChat() {
            chatHeld = java.util.concurrent.CountDownLatch(1)
        }

        fun releaseChat() {
            chatHeld?.countDown()
        }

        /** An event from the host. */
        fun push(line: String) {
            listener?.onBytes((line + "\n").encodeToByteArray())
        }

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                requests += request
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val held = scene.holdAnswers && op == "respond"
                val chatGate = if (op == "chat") chatHeld else null
                val refused = scene.refuseFirstAnswer && op == "respond" && sent("respond").size == 1
                val body = (if (refused) """{"ok":false,"error":"The agent stopped listening"}""" else reply(op))
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread {
                    if (held) answersHeld.await()
                    chatGate?.await()
                    Thread.sleep(5)
                    listener?.onBytes((body + "\n").encodeToByteArray())
                }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"settlement":true,"chat_quotas":true,"folders":[{"id":"folder","name":"xd"}],""" +
                """"sessions":[],"chats":[{"id":"chat","folder":"folder","title":"Ship the 2.4 release",""" +
                """"backend":"claude","working":false,"quota_reached":${scene.quotaReached}}]}"""
            "chat" -> """{"ok":true,"title":"Ship the 2.4 release","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"items":[],"new_worktree":false,"has_messages":true,""" +
                """"model":"claude-opus-5-5","effort":"high","draft":${JsonPrimitive(draft)},"draft_revision":1""" +
                (if (scene.interactions.isNotEmpty()) ""","interactions":${scene.interactions}""" else "") +
                (if (scene.quotaReached) ""","quota_reached":true,"quota":{"percent":10,"used":12.2,"own":false}""" else "") + "}"
            "messages" -> """{"ok":true,"total_messages":${scene.messages.size},"last_message_id":${scene.messages.size},"messages":[""" +
                scene.messages.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val TAGGED = listOf(
            "user" to "Cut the 2.4 release and get it out.",
            "assistant" to "The release is ready: the changelog is written and the build passed.\n\n" +
                "<ask>\nHow should we roll out this release?\n- Start with a small pilot\n- Release to everyone\n<input>\n</ask>",
        )

        val DRY_RUN = listOf(
            "user" to "Cut the 2.4 release and get it out.",
            "assistant" to "The changelog is written and the build passed. I'll do a dry run first to check the package before publishing.",
        )

        val QUESTIONS = """[{"request_id":"ask-1","method":"item/tool/requestUserInput","params":{"questions":[""" +
            """{"id":"rollout","question":"How should we roll out this release?",""" +
            """"options":[{"label":"Start with a small pilot"},{"label":"Release to everyone"}]},""" +
            """{"id":"when","question":"When should it go out?","options":[{"label":"Now"},{"label":"Tomorrow morning"}]}]}}]"""

        val APPROVALS = """[{"request_id":"approve-1","method":"item/commandExecution/requestApproval",""" +
            """"params":{"command":"cargo publish --dry-run --allow-dirty",""" +
            """"reason":"Checks that the crate packages cleanly. Nothing is uploaded."}},""" +
            """{"request_id":"approve-2","method":"item/commandExecution/requestApproval",""" +
            """"params":{"command":"git push origin v2.4.0"}}]"""
    }
}
