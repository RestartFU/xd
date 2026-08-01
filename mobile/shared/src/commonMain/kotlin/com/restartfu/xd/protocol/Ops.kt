package com.restartfu.xd.protocol

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
    BACKEND("backend"),
    NEW_WORKTREE("new-worktree"),
    WORKSPACE("workspace"),
}

public object Ops {
    public fun pair(code: String, name: String): JsonObject = buildJsonObject {
        put("op", "pair")
        put("code", code)
        put("name", name)
    }

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

    public fun newChat(
        folderId: String,
        title: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "new-chat")
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        put("folder", folderId)
        if (!title.isNullOrBlank()) put("title", title)
    }

    /** The assistants and models this daemon can run. */
    public fun agentCatalog(): JsonObject = op("agent-catalog")

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

    private fun op(name: String): JsonObject = buildJsonObject {
        put("op", name)
    }

    private fun withChat(name: String, chatId: String): JsonObject = buildJsonObject {
        put("op", name)
        put("chat", chatId)
    }
}
