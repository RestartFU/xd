package com.restartfu.xd.credentials

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class PairingInvitationTest {
    // The same sample as transport-core's tests, so both parsers read one format.
    private val code = "xd2:ABCD-EFGH:q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s"

    @Test
    fun readsTheCodeAndChosenAddress() {
        val invitation = parsePairingInvitation("100.64.0.1", code)
        assertEquals("100.64.0.1", invitation.host)
        assertEquals(4001, invitation.port)
        assertEquals("ABCD-EFGH", invitation.code)
        assertEquals("ab".repeat(32), invitation.fingerprint)
    }

    @Test
    fun survivesLineWrapping() {
        val wrapped = "  ${code.take(20)}\n${code.drop(20)}\n"
        assertEquals("ab".repeat(32), parsePairingInvitation("host", wrapped).fingerprint)
    }

    @Test
    fun rejectsMalformedCodes() {
        for (text in listOf(
            "",
            "ABCD-EFGH",
            "xd2:ABCD-EFGH",
            "xd2::q6urq6urq6urq6urq6urq6urq6urq6urq6urq6urq6s",
            "xd2:ABCD-EFGH:q6ur",
            "xd2:ABCD-EFGH:not base64!",
        )) {
            assertFailsWith<IllegalArgumentException>(text) { parsePairingInvitation("host", text) }
        }
        val older = assertFailsWith<IllegalArgumentException> { parsePairingInvitation("host", "xd1:abc") }
        assertTrue(older.message!!.contains("older version"))
    }

    @Test
    fun addressesDefaultToThePairingPort() {
        for ((text, expected) in listOf(
            "100.64.0.1" to ("100.64.0.1" to 4001),
            " 100.64.0.1:4444 " to ("100.64.0.1" to 4444),
            "vps.tailnet.ts.net" to ("vps.tailnet.ts.net" to 4001),
            "vps:9" to ("vps" to 9),
            "fd7a:115c::1" to ("fd7a:115c::1" to 4001),
            "[fd7a:115c::1]" to ("fd7a:115c::1" to 4001),
            "[fd7a:115c::1]:4444" to ("fd7a:115c::1" to 4444),
        )) {
            assertEquals(expected, parsePairingAddress(text), text)
        }
        for (text in listOf("", "   ", "host:0", "host:70000", "host:port", "a b", "[::1", "[::1]4001", "[]:4001", ":4001")) {
            assertFailsWith<IllegalArgumentException>(text) { parsePairingAddress(text) }
        }
    }
}
