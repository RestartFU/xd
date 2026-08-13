package com.restartfu.xd.terminal

import com.restartfu.xd.protocol.TerminalReply
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull

/** A terminal event, already decoded off the wire. */
public sealed interface TerminalEvent {
    public val chatId: String
    public val terminalId: String

    public data class Output(
        override val chatId: String,
        override val terminalId: String,
        val data: ByteArray,
    ) : TerminalEvent {
        override fun equals(other: Any?): Boolean =
            this === other ||
                (
                    other is Output &&
                        chatId == other.chatId &&
                        terminalId == other.terminalId &&
                        data.contentEquals(other.data)
                    )

        override fun hashCode(): Int =
            (chatId.hashCode() * 31 + terminalId.hashCode()) * 31 + data.contentHashCode()
    }

    public data class Closed(
        override val chatId: String,
        override val terminalId: String,
    ) : TerminalEvent
}

/** One frame of the host's bounded output history. */
public sealed interface ReplayFrame {
    public data class Output(val data: ByteArray) : ReplayFrame {
        override fun equals(other: Any?): Boolean =
            this === other || (other is Output && data.contentEquals(other.data))

        override fun hashCode(): Int = data.contentHashCode()
    }

    public data class Resize(val columns: Int, val rows: Int) : ReplayFrame
}

@OptIn(ExperimentalEncodingApi::class)
public object TerminalWire {
    /**
     * Reads a `terminal-output` or `terminal-closed` event, or null when this
     * is neither. Base64 is decoded here so no caller handles wire encoding.
     */
    public fun event(value: JsonObject): TerminalEvent? {
        val chat = value.text("chat") ?: return null
        val terminal = value.text("terminal") ?: return null
        return when (value.text("event")) {
            "terminal-output" -> {
                val data = value.text("data") ?: return null
                val bytes = runCatching { Base64.Default.decode(data) }.getOrNull()
                bytes?.let { TerminalEvent.Output(chat, terminal, it) }
            }
            "terminal-closed" -> TerminalEvent.Closed(chat, terminal)
            else -> null
        }
    }

    /**
     * The replay history in order. Resize frames are kept between output
     * frames on purpose: older output has to be interpreted at the geometry
     * that produced it, not at today's size.
     */
    public fun frames(reply: TerminalReply): List<ReplayFrame> =
        reply.replay.mapNotNull { element ->
            val frame = element as? JsonObject ?: return@mapNotNull null
            frame.text("data")?.let { data ->
                return@mapNotNull runCatching { Base64.Default.decode(data) }
                    .getOrNull()
                    ?.let(ReplayFrame::Output)
            }
            val columns = frame.number("columns")
            val rows = frame.number("rows")
            if (columns != null && rows != null) ReplayFrame.Resize(columns, rows) else null
        }

    /** The pty takes bytes, and the wire takes base64. */
    public fun encode(bytes: ByteArray): String = Base64.Default.encode(bytes)

    private fun JsonObject.text(name: String): String? =
        (this[name] as? JsonPrimitive)?.contentOrNull

    private fun JsonObject.number(name: String): Int? =
        (this[name] as? JsonPrimitive)?.takeUnless { it.isString }?.intOrNull
}
