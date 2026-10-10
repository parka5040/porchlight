package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.TrustController
import app.porchlight.shared.trust.TrustState
import io.ktor.http.encodeURLQueryComponent
import kotlinx.coroutines.*
import kotlin.coroutines.AbstractCoroutineContextElement
import kotlin.coroutines.CoroutineContext
import kotlinx.serialization.json.*

/** Explicit application-owned background work only; never installed in a UI scope. */
private class BackgroundSession(val epoch: Long, val token: String?) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<BackgroundSession>
}
suspend fun <T> withBackgroundSession(controller: TrustController, block: suspend () -> T): T =
    withContext(BackgroundSession(controller.sessionEpoch.value, controller.sessionToken())) { block() }

/** Only sync reads and immutable notification actions may pass the UI-only PIN gate (R64). */
private fun backgroundRoute(method: String, path: String): Boolean = when (method) {
    "GET" -> path in setOf("/v1/snapshot", "/v1/board", "/v1/agenda", "/v1/members", "/v1/me/settings", "/v1/me/notification-prefs", "/v1/notifications", "/v1/items") ||
        Regex("/v1/items/[^/]+(?:/occurrences)?").matches(path) || Regex("/v1/occurrences/[^/]+").matches(path)
    "POST" -> Regex("/v1/occurrences/[^/]+/complete").matches(path) || Regex("/v1/notifications/[^/]+/read").matches(path)
    else -> false
}

/** Generated operations supply paths/models; C1b owns the only HTTP stack and all transport policy. */
class GeneratedCall(
    private val transport: () -> ApiTransport,
    private val session: () -> String?,
    private val beforeCall: () -> Unit = {},
    private val problem: suspend (ApiProblem) -> Unit = {},
    private val revoked: suspend (DeviceCertificateRevoked) -> Unit = {},
    private val epoch: () -> Long = { 0L },
    private val backgroundGate: (() -> Unit)? = null,
) {
    var onSuccess: () -> Unit = {}
    suspend fun request(method: String, path: String, query: Map<String, String>, headers: Map<String, String>, body: JsonObject?, representation: Boolean = false): JsonObject {
        val backgroundLease = currentCoroutineContext()[BackgroundSession.Key]
        fun leaseStale() = backgroundLease?.epoch?.let { it != epoch() || backgroundLease.token != session() } ?: false
        if (leaseStale()) throw CancellationException("Household session changed")
        val requestEpoch = backgroundLease?.epoch ?: epoch()
        val background = currentCoroutineContext()[BackgroundSession.Key] != null && backgroundGate != null &&
            backgroundRoute(method, path) && headers["Porchlight-Step-Up"] == null
        val gate = if (background) checkNotNull(backgroundGate) else beforeCall
        gate()
        val token = if (backgroundLease?.epoch != null) backgroundLease.token else session()
        if (leaseStale()) throw CancellationException("Household session changed")
        // A reply belongs to the identity epoch AND the session token it was sent with: a late reply/problem from an
        // ended identity, or from an expired session after a same-identity relogin, must not touch the new one.
        fun stale() = epoch() != requestEpoch || session() != token
        val suffix = query.entries.joinToString("&") { (k, v) -> k.encodeURLQueryComponent(encodeFull = true) + "=" + v.encodeURLQueryComponent(encodeFull = true) }
        try {
            val response = transport().requestReply(method, path + if (suffix.isEmpty()) "" else "?$suffix", body, token,
                headers["Idempotency-Key"], headers["If-Match"]?.trim('"')?.toLong(), headers["Porchlight-Step-Up"])
            if (stale()) throw CancellationException("Household session changed")
            // An in-flight response must not repopulate data after relock. The server may have committed a write.
            try { gate() } catch (e: HouseholdLocked) { throw HouseholdLockedAfterReply(headers["Idempotency-Key"]) }
            onSuccess()
            if (representation && response.status == 204) throw NoContentSuccess()
            return response.body
        } catch (p: ApiProblem) {
            if (stale()) throw CancellationException("Household session changed")
            problem(p); throw p
        } catch (e: DeviceCertificateRevoked) {
            if (epoch() != requestEpoch) throw CancellationException("Household session changed")
            revoked(e); throw e
        }
    }
    suspend fun download(path: String, stepUp: String?, consume: suspend (ByteArray) -> Unit, complete: suspend (() -> Unit) -> Unit = {}) {
        val requestEpoch = epoch()
        beforeCall()
        val token = session()
        fun checkGate() {
            if (epoch() != requestEpoch || session() != token) throw CancellationException("Household session changed")
            beforeCall()
        }
        try {
            transport().download(path, token, stepUp) { bytes -> checkGate(); consume(bytes) }
            checkGate()
            complete(::checkGate)
            checkGate()
        } catch (p: ApiProblem) { checkGate(); problem(p); throw p }
        catch (e: DeviceCertificateRevoked) {
            if (epoch() != requestEpoch) throw CancellationException("Household session changed")
            revoked(e); throw e
        }
    }
    fun ensureReady() = beforeCall()
    suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, key: String, progress: (Long, Long) -> Unit): JsonObject =
        binary(key) { t, token -> t.upload(path, name, mime, bytes, token, key, progress) }
    suspend fun download(id: String, thumbnail: Boolean = false): ByteArray =
        binary(null) { t, token -> t.download("/v1/attachments/$id" + if (thumbnail) "/thumbnail" else "", token) }
    private suspend fun <T> binary(key: String?, operation: suspend (ApiTransport, String?) -> T): T {
        val requestEpoch = epoch(); beforeCall(); val token = session()
        fun stale() = epoch() != requestEpoch || session() != token
        try {
            val result = operation(transport(), token)
            if (stale()) throw CancellationException("Household session changed")
            try { beforeCall() } catch (_: HouseholdLocked) { throw HouseholdLockedAfterReply(key) }
            return result
        } catch (p: ApiProblem) { if (stale()) throw CancellationException("Household session changed"); problem(p); throw p }
        catch (e: DeviceCertificateRevoked) { if (epoch() != requestEpoch) throw CancellationException("Household session changed"); revoked(e); throw e }
    }
    inline fun <reified T> decode(response: JsonObject): T = try { WireJson.decodeFromJsonElement<T>(response) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid API response") }

    companion object {
        fun trusted(controller: TrustController) = GeneratedCall(controller::transport, controller::sessionToken,
            { if (controller.state.value != TrustState.Ready) throw HouseholdLocked() },
            { controller.onProblem(it) }, { controller.onTransportError(it) }, { controller.sessionEpoch.value },
            { if (!controller.backgroundSessionUsable()) throw HouseholdLocked() })
    }
}
/** The trust gate is not Ready (PIN relock, expiry, re-enrollment); not a connectivity failure. Thrown as itself before
 * a request is sent: nothing reached the server, so a retry after unlocking is safe with any key. */
open class HouseholdLocked(message: String = "Unlock and sign in before using household data") : IllegalStateException(message)
/** The gate closed while the reply was in flight, so the reply was withheld. The outcome is AMBIGUOUS: the server may
 * have committed a write. After unlocking, retry a write only with the same body, version and [idempotencyKey] (the
 * repositories' `key` parameter), never with a new key; the server then replays the committed result. Reads simply
 * retry; their key is null. */
class HouseholdLockedAfterReply(val idempotencyKey: String?) :
    HouseholdLocked("The household locked before its reply arrived; the request may have been applied")
