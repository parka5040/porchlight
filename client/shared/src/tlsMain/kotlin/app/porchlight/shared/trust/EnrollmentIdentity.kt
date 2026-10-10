package app.porchlight.shared.trust

import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64

internal actual fun enrollmentBinding(input: String): String =
    MessageDigest.getInstance("SHA-256").digest(input.toByteArray(Charsets.UTF_8))
        .joinToString("") { "%02x".format(it) }

internal actual fun newTrustRequestKey(): String =
    Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(16).also { SecureRandom().nextBytes(it) })
