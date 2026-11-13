package com.restartfu.xd.mobile.ui

import android.util.Base64
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

/**
 * A scripted host for the accessibility tests and their captures: one project
 * tree with a session in each status, a chat with tool calls, a working
 * directory to browse, and a shared terminal that has printed in colour. It
 * answers each request from the script and can push events, as a host does.
 */
internal class AuditHost(
    /** Replaces the script's answer for a request; null leaves it to the script. */
    private val override: (op: String?, request: JsonObject) -> String? = { _, _ -> null },
    /** An event sent in the same read as the reply to [op], right after it; null sends none. */
    private val afterReply: (op: String?) -> String? = { null },
) {
    /** Every request the app sent, in order. */
    val sent = CopyOnWriteArrayList<JsonObject>()

    /** How many times the pairing was erased. */
    val forgotten = AtomicInteger()

    private val listener = AtomicReference<PlatformSocketListener?>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    val client = XdClient({ Socket() }, Credentials(), scope)

    fun connect(): AuditHost {
        val connected = runBlocking { client.connect(CONNECTION) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        return this
    }

    /** Sends an event, as the host does when something changes. */
    fun push(event: String) {
        listener.get()?.onBytes((event + "\n").encodeToByteArray())
    }

    /**
     * What the tree cannot say on its own: an approval waits in two chats,
     * and two turns finished while their chats were closed.
     */
    fun pushSignals() {
        for (chat in listOf("c1c", "c8")) {
            push("""{"event":"interaction","chat":"$chat","request_id":"r-$chat","method":"approval","params":{}}""")
        }
        for (chat in listOf("c3", "c9")) push("""{"event":"turn-finished","chat":"$chat"}""")
    }

    fun close() = scope.cancel()

    private inner class Credentials : CredentialStore {
        private var saved: StoredCredentials? = null
        override suspend fun load(): StoredCredentials? = saved
        override suspend fun save(credentials: StoredCredentials) {
            saved = credentials
        }

        override suspend fun clear() {
            saved = null
            forgotten.incrementAndGet()
        }
    }

    private inner class Socket : PlatformSocket {
        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this@AuditHost.listener.set(listener)
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                sent += request
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = (override(op, request) ?: reply(op, request))
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                val next = afterReply(op)?.let { it + "\n" }.orEmpty()
                Thread {
                    Thread.sleep(5)
                    listener.get()?.onBytes((body + "\n" + next).encodeToByteArray())
                }.start()
            }
        }

        override fun close() = Unit
    }

    companion object {
        private val CONNECTION = SshConnection(
            "host", 22, "dev", SshAuthentication.Password("x"),
            SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:test"),
        )

        private fun JsonObject.text(name: String): String? = (this[name] as? JsonPrimitive)?.content

        /** The script: what the host answers to [op]. */
        fun reply(op: String?, request: JsonObject): String = when (op) {
            "tree" -> TREE
            "chat" -> chat(request.text("chat").orEmpty())
            "messages" -> messages(if (request.text("chat") == "c1") MESSAGES else SHORT)
            "agent-catalog" -> CATALOG
            "git-status" -> """{"ok":true,"branch":"reconnect-backoff","base":"master","unstaged":2,"clean":false}"""
            "diff-read" -> """{"ok":true,"output":""}"""
            "file-browse" -> when (request.text("action")) {
                "read" -> """{"ok":true,"content":${JsonPrimitive(README)}}"""
                else -> entries(request.text("path").orEmpty())
            }
            "terminal-list" -> """{"ok":true,"terminals":[{"id":"t1","title":"shell","columns":80,"rows":24,""" +
                """"replay":[{"data":"${Base64.encodeToString(TERMINAL.encodeToByteArray(), Base64.NO_WRAP)}"}]}]}"""
            "auto-settle-settings" -> """{"ok":true,"after_days":3,"on_merge":true,"keep_pull_requests":false}"""
            else -> """{"ok":true}"""
        }

        private fun chat(id: String): String {
            val (title, backend) = CHATS[id] ?: ("New chat" to "claude")
            val model = if (backend == "claude") "claude-opus-5-5" else "gpt-6-luna"
            return """{"ok":true,"title":"$title","backend":"$backend","commands":[],"plan":false,"queue":[],""" +
                """"working":false,"items":[],"new_worktree":false,"has_messages":true,"model":"$model","effort":"high"}"""
        }

        private fun messages(rows: List<Pair<String, String>>): String =
            """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                rows.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"

        private fun entries(path: String): String {
            val names = when (path) {
                "" -> listOf("desktop/", "mobile/", "scripts/", "AGENTS.md", "DESIGN.md", "README.md")
                "mobile" -> listOf("androidApp/", "shared/", "build.gradle.kts", "settings.gradle.kts")
                else -> emptyList()
            }
            return """{"ok":true,"entries":[""" + names.joinToString(",") { name ->
                """{"name":"${name.trimEnd('/')}","directory":${name.endsWith('/')}}"""
            } + "]}"
        }

        private val CHATS = mapOf(
            "c1" to ("Fix reconnect backoff" to "claude"),
            "c1b" to ("Release notes" to "codex"),
            "c1c" to ("Flaky pairing test" to "codex"),
        )

        private fun session(id: String, folder: String, title: String, chats: List<String>): String =
            """{"id":"$id","folder":"$folder","title":"$title","chats":[${chats.joinToString(",") { "\"$it\"" }}],""" +
                """"terminal_chat":"session:$id","branch":"${title.lowercase().replace(' ', '-')}"}"""

        private fun chatRow(id: String, folder: String, title: String, backend: String, session: String, extra: String = ""): String =
            """{"id":"$id","folder":"$folder","title":"$title","backend":"$backend","working":false,"session":"$session"$extra}"""
                .let { if ("\"working\":true" in extra) it.replace("\"working\":false,", "") else it }

        /**
         * Two projects. xd holds a session in each status: waiting on the
         * reader, working, finished and unread, paused, waiting on background
         * tasks, and quiet. web holds one working, one waiting and one unread,
         * for its closed row to gather.
         */
        val TREE: String = """{"ok":true,"settlement":true,"folder_groups":true,"pause_all":true,"pause_folders":true,""" +
            """"linked_pull_requests":true,"folders":[{"id":"xd","name":"xd"},{"id":"web","name":"web"}],"sessions":[""" +
            listOf(
                session("s1", "xd", "Reconnect backoff", listOf("c1", "c1b", "c1c")),
                session("s2", "xd", "Usage page", listOf("c2")),
                session("s3", "xd", "Pairing flow", listOf("c3")),
                session("s4", "xd", "Diff pane", listOf("c4")),
                session("s5", "xd", "Theme tokens", listOf("c5")),
                session("s6", "xd", "Host update", listOf("c6")),
                session("s7", "web", "Landing copy", listOf("c7")),
                session("s8", "web", "Checkout fix", listOf("c8")),
                session("s9", "web", "Footer links", listOf("c9")),
            ).joinToString(",") + """],"chats":[""" +
            listOf(
                chatRow(
                    "c1", "xd", "Fix reconnect backoff", "claude", "s1",
                    ""","pull_requests":[{"url":"https://github.com/restartfu/xd/pull/205","state":"merged"},""" +
                        """{"url":"https://github.com/restartfu/xd/pull/212","state":"open"}]""",
                ),
                chatRow("c1b", "xd", "Release notes", "codex", "s1", ""","working":true"""),
                chatRow("c1c", "xd", "Flaky pairing test", "codex", "s1"),
                chatRow("c2", "xd", "Usage page", "codex", "s2", ""","working":true"""),
                chatRow("c3", "xd", "Pairing flow", "claude", "s3"),
                chatRow("c4", "xd", "Diff pane", "codex", "s4", ""","paused":true"""),
                chatRow("c5", "xd", "Theme tokens", "claude", "s5", ""","background_tasks":2"""),
                chatRow("c6", "xd", "Host update", "codex", "s6"),
                chatRow("c7", "web", "Landing copy", "codex", "s7", ""","working":true"""),
                chatRow("c8", "web", "Checkout fix", "claude", "s8"),
                chatRow("c9", "web", "Footer links", "claude", "s9"),
            ).joinToString(",") + "]}"

        private const val CATALOG = """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
            """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]},""" +
            """{"id":"codex","name":"Codex","default_model":"gpt-6-luna","models":[{"id":"gpt-6-luna","name":"Luna"}],"efforts":["high"]}]}"""

        /** A conversation with a run of tool calls, one call on its own, and a patch. */
        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off.",
            "assistant" to "I'll start with the connection actor, since that's where retries are scheduled.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "Grep \"reconnect\" in shared/src\nshared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt:581\n" +
                "shared/src/commonMain/kotlin/com/restartfu/xd/net/Backoff.kt:3",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/Backoff.kt",
            "assistant" to "Found it. `ConnectionActor` retries on a fixed timer and never resets after a good connection. " +
                "I'll replace that with capped backoff.",
            "tool" to "$ ./gradlew --offline :shared:testDebugUnitTest\nBUILD SUCCESSFUL in 41s\n359 tests completed",
            "assistant" to "## Result\n\nBackoff now starts at 2 s and grows to a 2 min cap. All 359 shared tests pass.",
        )

        private val SHORT = listOf(
            "user" to "Draft the release notes for this week.",
            "assistant" to "Here is a first draft, grouped by client.",
        )

        private const val README = "# xd\n\nA workspace for Codex and Claude sessions.\n"

        private const val ESC = "\u001B"

        /** A shell that has printed in each of the sixteen ANSI colours. */
        val TERMINAL: String = buildString {
            append("$ESC[32mdev@host$ESC[0m:$ESC[34m~/xd$ESC[0m$ git status --short\r\n")
            append(" $ESC[31mM$ESC[0m mobile/shared/Backoff.kt\r\n")
            append("$ESC[32mA$ESC[0m  mobile/shared/BackoffTest.kt\r\n")
            append("$ESC[32mdev@host$ESC[0m:$ESC[34m~/xd$ESC[0m$ ./gradlew test\r\n")
            append("$ESC[33mwarning$ESC[0m: 2 deprecated calls\r\n")
            append("$ESC[36m> Task$ESC[0m :shared:testDebugUnitTest\r\n")
            append("$ESC[35mBackoffTest$ESC[0m > growsToACap $ESC[1;32mPASSED$ESC[0m\r\n")
            append("$ESC[35mBackoffTest$ESC[0m > resets $ESC[1;91mFAILED$ESC[0m\r\n")
            append("$ESC[92mBUILD SUCCESSFUL$ESC[0m in 41s\r\n")
            append("\r\n")
            for (base in listOf(30, 90)) {
                for (code in base until base + 8) append("$ESC[${code}m███$ESC[0m ")
                append("\r\n")
            }
            for (base in listOf(30, 90)) {
                for (code in base until base + 8) append("$ESC[${code}mAa$ESC[0m  ")
                append("\r\n")
            }
            append("$ESC[32mdev@host$ESC[0m:$ESC[34m~/xd$ESC[0m$ ")
        }
    }
}
