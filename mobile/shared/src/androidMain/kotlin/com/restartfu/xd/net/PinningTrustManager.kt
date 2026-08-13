package com.restartfu.xd.net

import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.X509TrustManager

internal class PinMismatchCertificateException :
    CertificateException("The host certificate does not match the paired machine")

/**
 * Trusts exactly one leaf certificate. A null pin is allowed only by the
 * connection actor's pairing path and reports the offered leaf for TOFU.
 */
internal class PinningTrustManager(
    pinnedCertificateDer: ByteArray?,
) : X509TrustManager {
    private val pin = pinnedCertificateDer?.copyOf()

    override fun checkClientTrusted(
        chain: Array<out X509Certificate>?,
        authType: String?,
    ) {
        throw CertificateException("Client certificates are not accepted")
    }

    override fun checkServerTrusted(
        chain: Array<out X509Certificate>?,
        authType: String?,
    ) {
        val leaf = chain?.firstOrNull()
            ?: throw CertificateException("The host supplied no certificate")
        val expected = pin ?: return
        if (!leaf.encoded.contentEquals(expected)) {
            throw PinMismatchCertificateException()
        }
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}
