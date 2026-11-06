package com.restartfu.xd.credentials

import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi

private const val DEFAULT_PAIRING_PORT: Int = 4001
private const val PIN_BYTES: Int = 32

/** The address the user chose, combined with the code the host handed out. */
public class PairingInvitation(
    public val host: String,
    public val port: Int,
    public val fingerprint: String,
    public val code: String,
)

/**
 * Reads `xd2:<one-use code>:<base64url certificate SHA-256>` plus an address.
 * Matches `PairingInvite` in transport-core. Throws IllegalArgumentException
 * with a message for the user when either input is invalid.
 */
@OptIn(ExperimentalEncodingApi::class)
public fun parsePairingInvitation(address: String, pairingCode: String): PairingInvitation {
    val (host, port) = parsePairingAddress(address)
    require(pairingCode.length <= 4096) { "Pairing code is too long." }
    // Long codes can pick up line breaks when copied out of a terminal.
    val text = pairingCode.filterNot(Char::isWhitespace)
    require(!text.startsWith("xd1:")) {
        "This pairing code is from an older version of xd. Update the host and create a new code."
    }
    require(text.startsWith("xd2:")) { "Paste the full pairing code copied from the host." }
    val parts = text.removePrefix("xd2:").split(":")
    require(parts.size == 2 && parts[0].isNotEmpty() && parts[0].length <= 64) { "Invalid pairing code." }
    val pin = try {
        Base64.UrlSafe.withPadding(Base64.PaddingOption.ABSENT).decode(parts[1])
    } catch (_: IllegalArgumentException) {
        throw IllegalArgumentException("Invalid pairing code.")
    }
    require(pin.size == PIN_BYTES) { "Invalid pairing code." }
    val fingerprint = pin.joinToString("") { (it.toInt() and 0xff).toString(16).padStart(2, '0') }
    return PairingInvitation(host, port, fingerprint, parts[0])
}

/** Reads `host`, `host:port`, an IPv6 literal, or `[ipv6]:port`. */
public fun parsePairingAddress(address: String): Pair<String, Int> {
    val text = address.trim()
    require(text.isNotEmpty()) { "Enter the host's LAN or Tailscale address." }
    val invalid = "Invalid host address or port."
    val host: String
    val port: String?
    if (text.startsWith("[")) {
        val end = text.indexOf(']')
        require(end > 0) { invalid }
        host = text.substring(1, end)
        val rest = text.substring(end + 1)
        require(rest.isEmpty() || rest.startsWith(":")) { invalid }
        port = rest.removePrefix(":").takeIf { rest.isNotEmpty() }
    } else if (text.count { it == ':' } == 1) {
        host = text.substringBefore(':')
        port = text.substringAfter(':')
    } else {
        // No colon at all, or an unbracketed IPv6 literal.
        host = text
        port = null
    }
    val number = if (port == null) DEFAULT_PAIRING_PORT else port.toIntOrNull() ?: throw IllegalArgumentException(invalid)
    require(host.isNotEmpty() && host.length <= 253 && host.none(Char::isWhitespace) && number in 1..65535) { invalid }
    return host to number
}
