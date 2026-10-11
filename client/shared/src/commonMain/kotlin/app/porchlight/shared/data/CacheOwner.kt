package app.porchlight.shared.data

import app.porchlight.shared.trust.DeviceTrust

/** Non-secret cache identity. Enrollment learns the member from /me; until then the pinned
 * household + device certificate uniquely bind it. Member discovery fills, never changes, that identity. */
@kotlinx.serialization.Serializable
data class CacheOwner(val caFingerprint: String, val memberId: String?, val deviceId: String)
internal fun CacheOwner.owns(next: CacheOwner) = caFingerprint == next.caFingerprint && deviceId == next.deviceId &&
    (memberId == null || memberId == next.memberId)
internal fun DeviceTrust.cacheOwner() = CacheOwner(cacheCaFingerprint(caCertificate), memberId, deviceId)
internal expect fun cacheCaFingerprint(certificate: String): String
internal class CacheOwnerUnavailable : Exception("Household cache identity unavailable")
