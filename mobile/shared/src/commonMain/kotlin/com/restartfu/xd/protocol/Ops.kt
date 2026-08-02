package com.restartfu.xd.protocol

import com.restartfu.xd.automaticDeviceName
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.add
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

public enum class ChatOption(
    public val wire: String,
) {
    MODEL("model"),
    EFFORT("effort"),
    ACCESS("access"),
    PLAN("plan"),
    FAST("fast"),
    CLAUDE_MODE("claude-mode"),
    BACKEND("backend"),
    NEW_WORKTREE("new-worktree"),
    WORKSPACE("workspace"),
}

public object Ops {
    public fun pair(code: String, deviceName: String): JsonObject {
        require(deviceName.isNotBlank()) { "Device name must not be blank" }
        return buildJsonObject {
            put("op", "pair")
            put("code", code)
            put("name", deviceName)
        }
    }

    @Deprecated(
        "Pairing uses the connecting platform's automatic device name.",
        ReplaceWith("pair(code, automaticDeviceName())"),
    )
    public fun pair(code: String): JsonObject =
        pair(code, automaticDeviceName())

    public fun hello(token: String): JsonObject = buildJsonObject {
        put("op", "hello")
        put("token", token)
    }

    public fun tree(): JsonObject = op("tree")

    public fun chat(chatId: String): JsonObject = withChat("chat", chatId)

    public fun messages(chatId: String, limit: Int = 150): JsonObject = buildJsonObject {
        put("op", "messages")
        put("chat", chatId)
        put("limit", limit)
    }

    @OptIn(ExperimentalEncodingApi::class)
    public fun send(
        chatId: String,
        text: String,
        images: List<PngAttachment> = emptyList(),
    ): JsonObject {
        require(text.isNotEmpty() || images.isNotEmpty()) {
            "A message needs text or an image"
        }
        Limits.validateImages(images)

        return buildJsonObject {
            put("op", "send")
            put("chat", chatId)
            put("text", text)
            if (images.isNotEmpty()) {
                put(
                    "attachments",
                    buildJsonArray {
                        images.forEach { image ->
                            val encodedLength = ((image.bytes.size + 2) / 3) * 4
                            Limits.validateEncodedImageLength(encodedLength)
                            add(
                                buildJsonObject {
                                    put("mime", Limits.PNG_MIME)
                                    put("data", Base64.Default.encode(image.bytes))
                                },
                            )
                        }
                    },
                )
            }
        }
    }

    public fun queue(chatId: String, text: String): JsonObject = buildJsonObject {
        require(text.isNotEmpty()) { "Queued text must not be empty" }
        put("op", "queue")
        put("chat", chatId)
        put("text", text)
    }

    public fun dropQueue(chatId: String, index: Int?): JsonObject = buildJsonObject {
        put("op", "drop-queue")
        put("chat", chatId)
        if (index != null) {
            require(index >= 0) { "Queue index must not be negative" }
            put("index", index)
        }
    }

    /**
     * Replaces a queued message.
     *
     * [oldText] is what the client believes is there. The daemon uses it to
     * refuse an edit aimed at a queue another device has already changed.
     */
    public fun editQueue(
        chatId: String,
        index: Int,
        oldText: String,
        text: String,
    ): JsonObject = buildJsonObject {
        require(index >= 0) { "Queue index must not be negative" }
        require(text.isNotEmpty()) { "Edited text must not be empty" }
        put("op", "edit-queue")
        put("chat", chatId)
        put("index", index)
        put("old-text", oldText)
        put("text", text)
    }

    /**
     * Promotes a queued message to the front and stops the running turn, so
     * the agent takes it up immediately instead of finishing first.
     *
     * [text] must be the message currently at [index]; the daemon refuses the
     * steer otherwise rather than redirecting the agent to the wrong thing.
     */
    public fun steerQueue(
        chatId: String,
        index: Int,
        text: String,
    ): JsonObject = buildJsonObject {
        require(index >= 0) { "Queue index must not be negative" }
        put("op", "steer-queue")
        put("chat", chatId)
        put("index", index)
        put("text", text)
    }

    public fun cancel(chatId: String): JsonObject = withChat("cancel", chatId)

    /**
     * Deletes a chat and everything it owns.
     *
     * The daemon forgets the agent session, kills the chat's terminals and
     * removes the stored transcript. There is no undo, so a client should ask
     * first.
     */
    public fun deleteChat(chatId: String): JsonObject = withChat("delete-chat", chatId)

    /** Renames a chat. The daemon refuses a blank title rather than clearing it. */
    public fun renameChat(chatId: String, title: String): JsonObject = buildJsonObject {
        require(title.isNotBlank()) { "A chat needs a title" }
        put("op", "rename-chat")
        put("chat", chatId)
        put("title", title)
    }

    public fun newChat(
        folderId: String,
        title: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "new-chat")
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        put("folder", folderId)
        if (!title.isNullOrBlank()) put("title", title)
    }

    /**
     * Reads a stored image.
     *
     * The daemon only serves paths inside its own remote-paste directory, so
     * this cannot be pointed at arbitrary files. [preview] asks for a scaled
     * copy, which is all a transcript needs.
     */
    public fun imageRead(path: String, preview: Boolean = true): JsonObject = buildJsonObject {
        require(path.isNotBlank()) { "An image path is required" }
        put("op", "image-read")
        put("path", path)
        put("preview", preview)
    }

    /** The assistants and models this daemon can run. */
    public fun agentCatalog(): JsonObject = op("agent-catalog")

    /**
     * Asks about, or performs, an update of the daemon itself.
     *
     * `install` replaces the files, which is safe while turns run; `restart`
     * drops every attached device and loses the running turn, so it is a
     * separate action nobody takes by accident.
     */
    public fun daemonUpdate(action: String = "status"): JsonObject = buildJsonObject {
        require(action in DAEMON_UPDATE_ACTIONS) { "No such daemon-update action" }
        put("op", "daemon-update")
        put("action", action)
    }

    /**
     * Selects an assistant and model together.
     *
     * The daemon only validates and reconciles when both travel in one
     * request: it checks the model belongs to the backend, clears an effort
     * the new backend does not support, and records the visible switch. Sent
     * without a backend it would simply store the string.
     */
    public fun selectModel(
        chatId: String,
        backend: String,
        model: String,
    ): JsonObject = buildJsonObject {
        require(backend.isNotBlank()) { "A model needs its assistant" }
        require(model.isNotBlank()) { "A model id is required" }
        put("op", "set-option")
        put("chat", chatId)
        put("option", ChatOption.MODEL.wire)
        put("backend", backend)
        put("value", model)
    }

    /**
     * Whether the speech model is installed on the daemon.
     *
     * Transcription runs where the chat runs, so this asks about that machine's
     * disk, not the phone's.
     */
    public fun voiceModel(chatId: String): JsonObject = withChat("voice-model", chatId)

    /**
     * Fetches the speech model onto the daemon. Progress arrives as `voice`
     * events carrying [token], not in the reply.
     */
    public fun voiceModelDownload(chatId: String, token: String): JsonObject = buildJsonObject {
        put("op", "voice-model-download")
        put("chat", chatId)
        put("request", validToken(token))
    }

    /**
     * Transcribes a recording. [audio] is a base64 WAV -- see
     * `com.restartfu.xd.voice.Wav` for the only shape the daemon's whisper
     * accepts.
     *
     * The reply says only that the job started; the text arrives as a `voice`
     * event, because transcription takes far longer than a request may.
     */
    public fun voiceTranscribe(
        chatId: String,
        token: String,
        audio: String,
    ): JsonObject = buildJsonObject {
        require(audio.isNotEmpty()) { "A recording is required" }
        put("op", "voice-transcribe")
        put("chat", chatId)
        put("request", validToken(token))
        put("audio", audio)
    }

    /** Stops a download or transcription started with [token]. */
    public fun voiceCancel(token: String): JsonObject = buildJsonObject {
        put("op", "voice-cancel")
        put("request", validToken(token))
    }

    public fun ping(): JsonObject = op("ping")

    public fun setOption(
        chatId: String,
        option: ChatOption,
        value: String,
    ): JsonObject = buildJsonObject {
        put("op", "set-option")
        put("chat", chatId)
        put("option", option.wire)
        put("value", value)
    }

    public fun setBoolOption(
        chatId: String,
        option: ChatOption,
        value: Boolean,
    ): JsonObject = setOption(chatId, option, if (value) "true" else "false")

    /**
     * Reads a whole patch in one call.
     *
     * The desktop also asks per file to build its collapsible sections; a
     * phone shows one scrollable patch, so `working-all` and `branch-all` are
     * all it needs. `branch-all` wants the base from a prior `base` read.
     */
    public fun diffRead(
        chatId: String,
        read: String,
        base: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "diff-read")
        put("chat", chatId)
        put("read", read)
        if (!base.isNullOrBlank()) put("base", base)
    }

    public fun listDirectory(chatId: String, path: String?): JsonObject = buildJsonObject {
        put("op", "file-browse")
        put("chat", chatId)
        put("action", "list")
        if (!path.isNullOrEmpty()) put("path", path)
    }

    public fun readFile(chatId: String, path: String): JsonObject = buildJsonObject {
        require(path.isNotBlank()) { "A file path is required" }
        put("op", "file-browse")
        put("chat", chatId)
        put("action", "read")
        put("path", path)
    }

    public fun terminalList(chatId: String): JsonObject = withChat("terminal-list", chatId)

    public fun terminalOpen(
        chatId: String,
        columns: Int,
        rows: Int,
        reuse: Boolean,
    ): JsonObject = buildJsonObject {
        require(columns > 0 && rows > 0) { "A terminal needs a positive size" }
        put("op", "terminal-open")
        put("chat", chatId)
        put("columns", columns)
        put("rows", rows)
        put("reuse", reuse)
    }

    /** [data] is base64: the pty takes bytes, not text. */
    public fun terminalInput(terminalId: String, data: String): JsonObject = buildJsonObject {
        put("op", "terminal-input")
        put("terminal", terminalId)
        put("data", data)
    }

    public fun terminalResize(
        terminalId: String,
        columns: Int,
        rows: Int,
    ): JsonObject = buildJsonObject {
        require(columns > 0 && rows > 0) { "A terminal needs a positive size" }
        put("op", "terminal-resize")
        put("terminal", terminalId)
        put("columns", columns)
        put("rows", rows)
    }

    public fun terminalKill(terminalId: String): JsonObject = buildJsonObject {
        put("op", "terminal-kill")
        put("terminal", terminalId)
    }

    private val DAEMON_UPDATE_ACTIONS = setOf("status", "check", "install", "restart")

    /** The daemon keys voice jobs on this, and refuses anything longer. */
    private const val MAX_TOKEN_BYTES = 128

    private fun validToken(token: String): String {
        val cleaned = token.trim()
        require(cleaned.isNotEmpty()) { "A voice request needs a token" }
        require(cleaned.encodeToByteArray().size <= MAX_TOKEN_BYTES) {
            "That voice token is too long"
        }
        return cleaned
    }

    private fun op(name: String): JsonObject = buildJsonObject {
        put("op", name)
    }

    private fun withChat(name: String, chatId: String): JsonObject = buildJsonObject {
        put("op", name)
        put("chat", chatId)
    }
}
