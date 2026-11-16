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
private class BackgroundSession(val epoch: Long, val token: String?, val revision: Long) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<BackgroundSession>
}
suspend fun <T> withBackgroundSession(controller: TrustController, block: suspend () -> T): T =
    withContext(BackgroundSession(controller.sessionEpoch.value, controller.sessionToken(), controller.sessionRevision.value)) { block() }

/** Pins scheduled reads without granting the background PIN bypass to UI callers. */
internal class ReadSession(val epoch: Long, val token: String?, val revision: Long, val currentRevision: () -> Long) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<ReadSession>
}

/** Immutable origin of a pending write, inherited by every step and identical retry. */
internal class WriteOrigin(val epoch: Long, val token: String?, val revision: Long) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<WriteOrigin>
}

/** Only sync reads and immutable notification actions may pass the UI-only PIN gate (R64). */
private fun backgroundRoute(method: String, path: String): Boolean = when (method) {
    "GET" -> path in setOf("/v1/snapshot", "/v1/board", "/v1/agenda", "/v1/members", "/v1/me/settings", "/v1/me/notification-prefs", "/v1/notifications", "/v1/items", "/v1/lists", "/v1/suggestions", "/v1/feeds") ||
        Regex("/v1/members/[^/]+/availability").matches(path) ||
        Regex("/v1/items/[^/]+(?:/occurrences|/sync|/comments)?").matches(path) || Regex("/v1/occurrences/[^/]+").matches(path) ||
        Regex("/v1/lists/[^/]+").matches(path)
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
    @PublishedApi internal val jsonDispatcher: CoroutineDispatcher = Dispatchers.Default,
    private val sessionRevision: () -> Long = { 0L },
) {
    var onSuccess: () -> Unit = {}
    internal fun writeOrigin() = WriteOrigin(epoch(), session(), sessionRevision())
    internal fun currentSessionRevision() = sessionRevision()
    private suspend fun checkWriteOrigin() {
        currentCoroutineContext()[WriteOrigin]?.let {
            if (it.epoch != epoch() || it.token != session() || it.revision != sessionRevision())
                throw CancellationException("Pending request belongs to another household session")
        }
    }
    private suspend fun checkReadSession() {
        checkWriteOrigin()
        currentCoroutineContext()[ReadSession]?.let {
            if (it.epoch != epoch() || it.token != session() || it.revision != sessionRevision())
                throw CancellationException("Household session changed")
        }
    }
    internal suspend fun readSession(): ReadSession {
        val context = currentCoroutineContext()
        val background = context[BackgroundSession]
        return context[ReadSession] ?: ReadSession(background?.epoch ?: epoch(),
            if (background != null) background.token else session(), background?.revision ?: sessionRevision(), sessionRevision)
    }
    internal suspend fun <T> withReadSession(block: suspend (Long) -> T): T {
        val admitted = readSession()
        return withContext(admitted) { checkReadSession(); block(admitted.revision) }
    }
    suspend fun request(method: String, path: String, query: Map<String, String>, headers: Map<String, String>, body: JsonObject?, representation: Boolean = false): JsonObject =
        requestRepeated(method, path, query.mapValues { listOf(it.value) }, headers, body, representation)

    /** Exploded form arrays repeat keys; encode each value independently, including literal '+' offsets. */
    suspend fun requestRepeated(method: String, path: String, query: Map<String, List<String>>, headers: Map<String, String>, body: JsonObject?, representation: Boolean = false): JsonObject {
        currentCoroutineContext()[TargetedReadPolicy]?.before?.invoke()
        checkReadSession()
        val readLease = currentCoroutineContext()[ReadSession]
        val backgroundLease = currentCoroutineContext()[BackgroundSession.Key]
        fun leaseStale() = backgroundLease?.epoch?.let {
            it != epoch() || backgroundLease.token != session() || backgroundLease.revision != sessionRevision()
        } ?: false
        if (leaseStale()) throw CancellationException("Household session changed")
        val writeLease = currentCoroutineContext()[WriteOrigin]
        val requestEpoch = writeLease?.epoch ?: readLease?.epoch ?: backgroundLease?.epoch ?: epoch()
        val background = currentCoroutineContext()[BackgroundSession.Key] != null && backgroundGate != null &&
            backgroundRoute(method, path) && headers["Porchlight-Step-Up"] == null
        val gate = if (background) checkNotNull(backgroundGate) else beforeCall
        gate()
        val token = if (writeLease != null) writeLease.token else if (readLease != null) readLease.token else if (backgroundLease != null) backgroundLease.token else session()
        checkWriteOrigin()
        if (leaseStale()) throw CancellationException("Household session changed")
        // A reply belongs to the identity epoch AND the session token it was sent with: a late reply/problem from an
        // ended identity, or from an expired session after a same-identity relogin, must not touch the new one.
        fun stale() = epoch() != requestEpoch || session() != token
        val suffix = query.entries.flatMap { (key, values) -> values.map { value ->
            key.encodeURLQueryComponent(encodeFull = true) + "=" + value.encodeURLQueryComponent(encodeFull = true)
        } }.joinToString("&")
        try {
            val target = transport()
            checkReadSession()
            val response = target.requestReply(method, path + if (suffix.isEmpty()) "" else "?$suffix", body, token,
                headers["Idempotency-Key"], headers["If-Match"]?.trim('"')?.toLong(), headers["Porchlight-Step-Up"])
            checkReadSession()
            if (stale()) throw CancellationException("Household session changed")
            // An in-flight response must not repopulate data after relock. The server may have committed a write.
            try { gate() } catch (e: HouseholdLocked) { throw HouseholdLockedAfterReply(headers["Idempotency-Key"]) }
            onSuccess()
            if (representation && response.status == 204) throw NoContentSuccess()
            return response.body
        } catch (p: ApiProblem) {
            checkReadSession()
            if (stale()) throw CancellationException("Household session changed")
            currentCoroutineContext()[TargetedReadPolicy]?.record(p)
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
    suspend fun downloadAttachment(id: String, consume: suspend (ByteArray) -> Unit) = binary(null) { t, token ->
        var received = 0L
        t.download("/v1/attachments/$id", token, null, null, { head ->
            if ((head.length ?: 0) > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
        }, { bytes ->
            received += bytes.size
            if (received > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
            beforeCall(); consume(bytes)
        })
    }
    suspend fun download(id: String, thumbnail: Boolean = false): ByteArray =
        binary(null) { t, token -> t.download("/v1/attachments/$id" + if (thumbnail) "/thumbnail" else "", token) }
    private suspend fun <T> binary(key: String?, operation: suspend (ApiTransport, String?) -> T): T {
        checkWriteOrigin()
        val origin = currentCoroutineContext()[WriteOrigin] ?: writeOrigin()
        val requestEpoch = origin.epoch; beforeCall(); val token = origin.token
        fun stale() = epoch() != requestEpoch || session() != token || sessionRevision() != origin.revision
        try {
            val target = transport()
            checkWriteOrigin()
            if (stale()) throw CancellationException("Household session changed")
            val result = operation(target, token)
            checkWriteOrigin()
            if (stale()) throw CancellationException("Household session changed")
            try { beforeCall() } catch (_: HouseholdLocked) { throw HouseholdLockedAfterReply(key) }
            return result
        } catch (p: ApiProblem) { if (stale()) throw CancellationException("Household session changed"); problem(p); throw p }
        catch (e: DeviceCertificateRevoked) { if (epoch() != requestEpoch) throw CancellationException("Household session changed"); revoked(e); throw e }
    }
    @PublishedApi internal suspend fun <T> encodeJson(block: () -> T): T {
        checkReadSession()
        val requestEpoch = epoch()
        val token = session()
        fun stale() = epoch() != requestEpoch || session() != token
        try {
            val value = withContext(jsonDispatcher) { block() }
            checkReadSession()
            if (stale()) throw CancellationException("Household session changed")
            return value
        } catch (e: Exception) {
            if (stale()) throw CancellationException("Household session changed")
            throw e
        }
    }
    suspend inline fun <reified T> encode(value: T): JsonObject? = encodeJson {
        WireJson.encodeToJsonElement(value).let { if (it is JsonNull) null else it.jsonObject }
    }
    @PublishedApi internal suspend fun <T> decodeJson(method: String, path: String, headers: Map<String, String>, block: () -> T): T {
        checkReadSession()
        val requestEpoch = epoch()
        val token = session()
        fun stale() = epoch() != requestEpoch || session() != token
        val background = currentCoroutineContext()[BackgroundSession.Key] != null && backgroundGate != null &&
            backgroundRoute(method, path) && headers["Porchlight-Step-Up"] == null
        val gate = if (background) checkNotNull(backgroundGate) else beforeCall
        try {
            val value = withContext(jsonDispatcher) { block() }
            checkReadSession()
            if (stale()) throw CancellationException("Household session changed")
            try { gate() } catch (_: HouseholdLocked) { throw HouseholdLockedAfterReply(headers["Idempotency-Key"]) }
            return value
        } catch (e: Exception) {
            if (stale()) throw CancellationException("Household session changed")
            throw e
        }
    }
    suspend inline fun <reified T> decode(response: JsonObject, method: String = "", path: String = "", headers: Map<String, String> = emptyMap()): T = decodeJson(method, path, headers) {
        try { WireJson.decodeFromJsonElement<T>(response) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid API response") }
    }

    companion object {
        fun trusted(controller: TrustController) = GeneratedCall(controller::transport, controller::sessionToken,
            { if (controller.state.value != TrustState.Ready) throw HouseholdLocked() },
            { controller.onProblem(it) }, { controller.onTransportError(it) }, { controller.sessionEpoch.value },
            { if (!controller.backgroundSessionUsable()) throw HouseholdLocked() },
            sessionRevision = { controller.sessionRevision.value })
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
