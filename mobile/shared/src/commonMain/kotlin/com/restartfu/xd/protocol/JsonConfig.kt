package com.restartfu.xd.protocol

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.decodeFromJsonElement

@OptIn(ExperimentalSerializationApi::class)
public val WireJson: Json = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = false
}

/**
 * Private request-id member the host echoes back on every reply.
 *
 * Mirrors `REQUEST_ID` in `host/src/lib.rs`. Sending it opts this client into reply
 * multiplexing, so a slow `diff-read` cannot hold a `cancel` behind it.
 */
public const val REQUEST_ID: String = "_xd_request"

public class RemoteRefusedException(
    message: String,
) : Exception(message)

public class RemoteProtocolException(
    message: String,
    cause: Throwable? = null,
) : Exception(message, cause)

public fun JsonObject.requireSuccess(): JsonObject {
    val ok = (this["ok"] as? JsonPrimitive)?.booleanOrNull
        ?: throw RemoteProtocolException("Remote reply has no boolean ok member")
    if (!ok) {
        val message = (this["error"] as? JsonPrimitive)?.contentOrNull
            ?: "The host refused the request."
        throw RemoteRefusedException(message)
    }
    return this
}

public inline fun <reified T> JsonObject.decodeReply(): T {
    requireSuccess()
    return try {
        WireJson.decodeFromJsonElement<T>(this)
    } catch (error: Throwable) {
        throw RemoteProtocolException("Remote reply has an invalid shape", error)
    }
}
