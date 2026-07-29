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

    public fun cancel(chatId: String): JsonObject = withChat("cancel", chatId)

    public fun newChat(
        folderId: String?,
        title: String? = null,
    ): JsonObject = buildJsonObject {
        put("op", "new-chat")
        if (folderId != null) put("folder", folderId)
        if (!title.isNullOrBlank()) put("title", title)
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

    private fun op(name: String): JsonObject = buildJsonObject {
        put("op", name)
    }

    private fun withChat(name: String, chatId: String): JsonObject = buildJsonObject {
        put("op", name)
        put("chat", chatId)
    }
}
