package android.util

/** Host-only equivalent of the two Android Base64 operations used by the encrypted trust store. */
object Base64 {
    const val NO_WRAP = 2
    @JvmStatic fun decode(value: String, @Suppress("UNUSED_PARAMETER") flags: Int): ByteArray = java.util.Base64.getDecoder().decode(value)
    @JvmStatic fun encodeToString(value: ByteArray, @Suppress("UNUSED_PARAMETER") flags: Int): String = java.util.Base64.getEncoder().encodeToString(value)
}
