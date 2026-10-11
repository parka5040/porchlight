package app.porchlight.shared.data

import java.security.MessageDigest
import java.util.Base64

/** Trust storage has already validated the CA. Hash DER, not PEM formatting or a leaf renewed in place. */
internal actual fun cacheCaFingerprint(certificate: String): String {
    val body = certificate.replace("-----BEGIN CERTIFICATE-----", "").replace("-----END CERTIFICATE-----", "")
        .filterNot(Char::isWhitespace)
    return MessageDigest.getInstance("SHA-256").digest(Base64.getDecoder().decode(body))
        .joinToString("") { "%02x".format(it) }
}
