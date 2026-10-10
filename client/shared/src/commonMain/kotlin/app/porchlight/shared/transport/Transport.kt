package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.request.*
import io.ktor.client.statement.bodyAsChannel
import io.ktor.utils.io.*
import io.ktor.client.statement.bodyAsText
import io.ktor.client.request.forms.*
import io.ktor.client.plugins.onUpload
import io.ktor.client.plugins.timeout
import kotlinx.coroutines.CancellationException
import io.ktor.http.*
import kotlinx.serialization.json.*

val WireJson = Json { ignoreUnknownKeys = true }
data class ClientInfo(val version: String)

class HeaderPolicy(private val info: ClientInfo, private val zone: () -> String?, private val randomKey: () -> String) {
    fun headers(method: String, session: String? = null, idempotencyKey: String? = null, version: Long? = null, stepUp: String? = null): Map<String, String> = buildMap {
        put("Porchlight-Client-Version", info.version)
        zone()?.let { put("Porchlight-TZ", it) }
        if (!method.equals("GET", true)) put("Idempotency-Key", idempotencyKey ?: randomKey())
        session?.let { put("Authorization", "Bearer $it") }
        version?.let { put("If-Match", it.toString()) }
        stepUp?.let { put("Porchlight-Step-Up", it) }
    }
}

class ApiProblem(val type: String, val status: Int, val title: String, val detail: String? = null, val extras: Map<String, JsonElement> = emptyMap(), val retryAfterMillis: Long? = null) : Exception("$status $type") {
    override fun toString() = "ApiProblem($status $type)" // no raw bodies/password/token echoes
    companion object {
        fun decode(body: String, retryAfterMillis: Long? = null): ApiProblem = try {
            val obj = WireJson.parseToJsonElement(body).jsonObject
            ApiProblem(obj.getValue("type").jsonPrimitive.content.substringAfterLast(':'), obj.getValue("status").jsonPrimitive.int,
                obj.getValue("title").jsonPrimitive.content, obj["detail"]?.jsonPrimitive?.contentOrNull,
                obj.filterKeys { it !in setOf("type", "status", "title", "detail") }, retryAfterMillis)
        } catch (_: Exception) {
            // kotlinx.serialization's parse exceptions echo input; never attach them as causes.
            throw ApiProblem("invalid-response", 502, "Invalid API problem response")
        }
    }
}

class DeviceCertificateRevoked(val requestNotSent: Boolean = false) : Exception("Device certificate revoked; re-enrollment required")

class RequestNotSent : Exception("TLS handshake failed before sending the request")

class ConnectFailure : Exception("Unable to connect to household endpoint")
data class JsonReply(val body: JsonObject, val status: Int = 200)
/** A committed success with no representation, not a problem or an ambiguous failure. */
class NoContentSuccess : Exception("Mutation succeeded without a representation")

interface ApiTransport {
    suspend fun request(method: String, path: String, body: JsonObject? = null, session: String? = null,
                        idempotencyKey: String? = null, version: Long? = null, stepUp: String? = null): JsonObject
    suspend fun stream(after: Long, session: String?, receive: suspend (app.porchlight.shared.live.StreamFrame) -> Unit) { throw ConnectFailure() }
    suspend fun requestReply(method: String, path: String, body: JsonObject? = null, session: String? = null,
        idempotencyKey: String? = null, version: Long? = null, stepUp: String? = null): JsonReply =
        JsonReply(request(method, path, body, session, idempotencyKey, version, stepUp))
    /** Stream through the same pinned mTLS client. A failure must abort the destination, never replay it. */
    suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
        throw UnsupportedOperationException("Download not supported by this transport")
    }
    suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, session: String?, key: String, progress: (Long, Long) -> Unit): JsonObject = throw UnsupportedOperationException("Binary uploads unavailable")
    /** Attachment-only adapter over the shared stream; never buffers more than the 25 MiB cap. */
    suspend fun download(path: String, session: String?): ByteArray {
        require(path.startsWith("/v1/attachments/"))
        val bytes = AttachmentBytes()
        download(path, session, null, null, { head ->
            if ((head.length ?: 0) > ATTACHMENT_LIMIT) throw attachmentTooLarge()
        }, { bytes.append(it, it.size) })
        return bytes.result()
    }
    /** Streaming binary read over the same root-only mTLS client; no ambiguous replay. */
    suspend fun download(path: String, session: String?, range: String?, ifRange: String?,
        begin: suspend (app.porchlight.shared.backups.DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
        throw UnsupportedOperationException("Binary downloads unavailable")
    }
    fun close() {}
}

fun orderedEndpoints(values: List<String>): List<String> = values.map { value ->
    val url = try { Url(value) } catch (_: Exception) { throw IllegalArgumentException("Invalid household endpoint") }
    require(url.protocol == URLProtocol.HTTPS && url.host.isNotEmpty() && url.user == null && url.password == null
        && url.encodedPath in listOf("", "/") && '?' !in value && '#' !in value) { "Household endpoint must be a secret-free HTTPS origin" }
    value.trimEnd('/')
}.distinct().sortedBy { Url(it).host.endsWith(".ts.net", ignoreCase = true) }

/** Caller supplies a root-only platform client. The body/headers/key are built once, before connect-only failover. */
class KtorApiTransport(
    private val client: HttpClient,
    endpoints: List<String>,
    private val policy: HeaderPolicy,
    private val connectFailure: (Throwable) -> Boolean,
    private val classifyFailure: (Exception) -> Exception = { it },
) : ApiTransport {
    private var endpoints = orderedEndpoints(endpoints)
    fun updateEndpoints(values: List<String>) { endpoints = orderedEndpoints(values) }
    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = requestReply(method, path, body, session, idempotencyKey, version, stepUp).body
    override suspend fun requestReply(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonReply {
        require(path.startsWith("/v1/"))
        val headers = policy.headers(method, session, idempotencyKey, version, stepUp)
        for ((index, endpoint) in endpoints.withIndex()) {
            val response = try {
                client.request(endpoint + path) {
                    this.method = HttpMethod.parse(method)
                    headers.forEach { (key, value) -> header(key, value) }
                    if (body != null) { contentType(ContentType.Application.Json); setBody(body.toString()) }
                }
            } catch (e: Exception) {
                // Never replay an ambiguous write/read failure, TLS failure, HTTP error or cancellation.
                if (!connectFailure(e)) throw classifyFailure(e)
                if (index == endpoints.lastIndex) throw ConnectFailure()
                continue
            }
            val text = try { response.bodyAsText() } catch (e: Exception) { throw classifyFailure(e) }
            if (response.status.value !in 200..299) {
                if (response.contentType()?.withoutParameters() == ContentType.parse("application/problem+json")) throw ApiProblem.decode(text, response.headers[HttpHeaders.RetryAfter]?.toLongOrNull()?.takeIf { it >= 0 }?.coerceAtMost(86_400)?.times(1000))
                throw ApiProblem("http-error", response.status.value, "Request failed")
            }
            val json = if (text.isBlank()) buildJsonObject {} else try { WireJson.parseToJsonElement(text).jsonObject }
                catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid API response") }
            return JsonReply(json, response.status.value)
        }
        throw ConnectFailure()
    }
    override suspend fun stream(after: Long, session: String?, receive: suspend (app.porchlight.shared.live.StreamFrame) -> Unit) {
        require(after >= 0)
        val headers = policy.headers("GET", session)
        for ((index, endpoint) in endpoints.withIndex()) {
            var opened = false
            try {
                client.prepareGet(endpoint + "/v1/stream") {
                    headers.forEach { (key, value) -> header(key, value) }
                    header(HttpHeaders.Accept, "text/event-stream")
                    header("Last-Event-ID", after.toString())
                }.execute { response ->
                    opened = true
                    if (response.status.value !in 200..299) {
                        val text = response.bodyAsText()
                        throw ApiProblem.decode(text)
                    }
                    if (response.contentType()?.withoutParameters() != ContentType.Text.EventStream) throw app.porchlight.shared.live.InvalidStream()
                    val parser = app.porchlight.shared.live.SseParser()
                    val channel = response.bodyAsChannel()
                    while (true) {
                        val line = kotlinx.coroutines.withTimeoutOrNull(app.porchlight.shared.live.STREAM_WATCHDOG_MS) { channel.readLineStrict(65536) ?: throw ConnectFailure() } ?: throw ConnectFailure()
                        parser.line(line)?.let { receive(it) }
                    }
                }
                return
            } catch (e: kotlinx.coroutines.CancellationException) { throw e }
            catch (e: Exception) {
                // Once a response opened, replay must use the acknowledged cursor, not another endpoint here.
                if (opened || !connectFailure(e)) throw classifyFailure(e)
                if (index == endpoints.lastIndex) throw ConnectFailure()
            }
        }
    }
    // c5: the same pinned client/header policy and connect-only failover as JSON, never a second HTTP stack.
    override suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, session: String?, key: String, progress: (Long, Long) -> Unit): JsonObject {
        require(path.startsWith("/v1/")); require(bytes.size <= 25 * 1024 * 1024)
        val safeName = name.substringAfterLast('/').substringAfterLast('\\').filter { it >= ' ' && it != '"' && it != '\r' && it != '\n' }.take(200).ifBlank { "attachment" }
        val data = MultiPartFormDataContent(formData {
            append("file", bytes, Headers.build {
                append(HttpHeaders.ContentType, mime)
                append(HttpHeaders.ContentDisposition, "filename=\"$safeName\"")
            })
        })
        for ((index, endpoint) in endpoints.withIndex()) {
            val response = try { client.post(endpoint + path) {
                policy.headers("POST", session, key).forEach { (k, v) -> header(k, v) }
                timeout { requestTimeoutMillis = 600_000; socketTimeoutMillis = 30_000 }
                setBody(data); onUpload { sent, total -> progress(sent, total ?: data.contentLength ?: bytes.size.toLong()) }
            } } catch (e: CancellationException) { throw e }
            catch (e: Exception) { if (!connectFailure(e)) throw classifyFailure(e); if (index == endpoints.lastIndex) throw ConnectFailure(); continue }
            val text = response.bodyAsText()
            if (response.status.value !in 200..299) throw binaryProblem(response.status.value, response.contentType(), text, response.headers[HttpHeaders.RetryAfter])
            return try { WireJson.parseToJsonElement(text).jsonObject } catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid upload reply") }
        }
        throw ConnectFailure()
    }
    override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
        require(path in setOf("/v1/export/member", "/v1/admin/export/household"))
        streamDownload(path, session, null, null, stepUp, {}, consume)
    }
    override suspend fun download(path: String, session: String?, range: String?, ifRange: String?,
        begin: suspend (app.porchlight.shared.backups.DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
        require(path == "/v1/backups/latest/download" || path.startsWith("/v1/attachments/") || path.startsWith("/v1/client-release/files/"))
        streamDownload(path, session, range, ifRange, null, begin, chunk)
    }
    // One streaming loop; export validation/caps belong to ExportRepository, not the attachment adapter.
    private suspend fun streamDownload(path: String, session: String?, range: String?, ifRange: String?, stepUp: String?,
        begin: suspend (app.porchlight.shared.backups.DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
        val attachment = path.startsWith("/v1/attachments/")
        val export = path in setOf("/v1/export/member", "/v1/admin/export/household")
        val headers = policy.headers("GET", session, stepUp = stepUp)
        for ((index, endpoint) in endpoints.withIndex()) {
            val statement = client.prepareGet(endpoint + path) {
                headers.forEach { (key, value) -> header(key, value) }
                range?.let { header(HttpHeaders.Range, it) }
                ifRange?.let { header(HttpHeaders.IfRange, it) }
            }
            var received = false
            try {
                // execute prevents eager buffering; callbacks and body errors must never replay.
                statement.execute { response ->
                    received = true
                    if (if (export) response.status.value !in 200..299 else response.status.value !in listOf(200, 206))
                        throw binaryProblem(response.status.value, response.contentType(), response.bodyAsText(), response.headers[HttpHeaders.RetryAfter], if (export) "Download failed" else "Request failed")
                    val length = response.headers[HttpHeaders.ContentLength]?.toLongOrNull()
                    if (attachment && (length ?: 0) > ATTACHMENT_LIMIT) throw attachmentTooLarge()
                    begin(app.porchlight.shared.backups.DownloadHead(response.status.value, response.headers[HttpHeaders.ETag],
                        length, response.headers[HttpHeaders.ContentRange]))
                    val channel = response.bodyAsChannel()
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val count = if (export) kotlinx.coroutines.withTimeout(300_000) {
                            channel.readAvailable(buffer, 0, buffer.size)
                        } else channel.readAvailable(buffer, 0, buffer.size)
                        if (count < 0) break
                        if (count > 0) {
                            total += count
                            if (attachment && total > ATTACHMENT_LIMIT) { channel.cancel(); throw attachmentTooLarge() }
                            chunk(buffer.copyOf(count))
                        }
                    }
                }
                return
            } catch (e: CancellationException) { throw e }
            catch (p: ApiProblem) { throw p }
            catch (e: Exception) {
                if (received || !connectFailure(e)) throw classifyFailure(e)
                if (index == endpoints.lastIndex) throw ConnectFailure()
            }
        }
        throw ConnectFailure()
    }
    private fun binaryProblem(status: Int, type: ContentType?, text: String, retry: String?, failureTitle: String = "Request failed"): ApiProblem =
        if (type?.withoutParameters() == ContentType.parse("application/problem+json")) ApiProblem.decode(text, retry?.toLongOrNull()?.takeIf { it >= 0 }?.coerceAtMost(86_400)?.times(1000))
        else ApiProblem("http-error", status, failureTitle)
    override fun close() { client.close() }
}

fun jsonBody(vararg fields: Pair<String, String>) = buildJsonObject { fields.forEach { (key, value) -> put(key, value) } }

private const val ATTACHMENT_LIMIT = 25 * 1024 * 1024
private fun attachmentTooLarge() = ApiProblem("attachment-too-large", 413, "Download exceeds limit")

/** Small thumbnails start at 8 KiB and grow only as bytes arrive. Check before copying, including
 * chunked replies and lying/missing Content-Length; no attachment-sized allocation up front. */
private class AttachmentBytes(private val limit: Int = ATTACHMENT_LIMIT) {
    init { require(limit in 1 until Int.MAX_VALUE) }
    private var buffer = ByteArray(minOf(8192, limit))
    private var count = 0
    fun append(bytes: ByteArray, size: Int) {
        if (size > limit - count) throw attachmentTooLarge()
        val needed = count + size
        if (needed > buffer.size)
            buffer = buffer.copyOf(minOf(limit.toLong(), maxOf(needed.toLong(), buffer.size.toLong() * 2)).toInt())
        bytes.copyInto(buffer, count, 0, size)
        count = needed
    }
    fun result(): ByteArray = buffer.copyOf(count)
}

internal suspend fun readAttachmentBytes(channel: ByteReadChannel, limit: Int = ATTACHMENT_LIMIT): ByteArray {
    val bytes = AttachmentBytes(limit)
    val buffer = ByteArray(minOf(8192, limit + 1))
    while (true) {
        val read = channel.readAvailable(buffer, 0, buffer.size)
        if (read < 0) return bytes.result()
        try { bytes.append(buffer, read) } catch (p: ApiProblem) { channel.cancel(); throw p }
    }
}
