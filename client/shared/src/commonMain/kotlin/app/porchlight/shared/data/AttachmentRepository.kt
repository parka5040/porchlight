package app.porchlight.shared.data

import app.porchlight.shared.api.apis.CollaborationApi
import app.porchlight.shared.api.apis.ItemsApi
import app.porchlight.shared.api.models.Attachment
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

const val MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024
class AttachmentTooLarge : IllegalArgumentException("Attachments must be at most 25 MiB")
fun ApiProblem.attachmentUploadMessage(): String? = if (status == 507 && type == "storage-full")
    "Attachment storage is full. Ask a household administrator to free space or increase the storage limit, then retry."
else null
data class UploadProgress(val sent: Long, val total: Long)
/** Keys contain only the persisted cache namespace, attachment UUID, version and thumbnail flag. */
interface AttachmentStore {
    suspend fun read(key: String): ByteArray?
    suspend fun write(key: String, bytes: ByteArray)
    suspend fun clear()
    suspend fun remove(key: String)
    suspend fun removeView(id: String) {}
    suspend fun cleanup(before: Instant)
    suspend fun content(key: String): AttachmentContent? = read(key)?.let(::ByteAttachmentContent)
    /** Optional disk-backed streaming stage; null selects the compatibility byte path. */
    suspend fun stage(key: String, receive: suspend (suspend (ByteArray) -> Unit) -> Unit): StagedAttachmentContent? = null
}
interface AttachmentContent {
    val privatePath: String? get() = null
    suspend fun chunks(consume: suspend (ByteArray) -> Unit)
    suspend fun bytes(): ByteArray
}
private class ByteAttachmentContent(private val value: ByteArray) : AttachmentContent {
    override suspend fun chunks(consume: suspend (ByteArray) -> Unit) { consume(value) }
    override suspend fun bytes() = value
}
interface StagedAttachmentContent : AttachmentContent {
    suspend fun publish()
    suspend fun discard()
}
/** Prepared privately off the cache/session locks; only launch is serialized with eviction. */
interface PreparedAttachmentViewer {
    suspend fun launch()
    suspend fun discard()
}
class MemoryAttachmentStore : AttachmentStore {
    private val rows = mutableMapOf<String, ByteArray>()
    override suspend fun read(key: String) = rows[key]?.copyOf()
    override suspend fun write(key: String, bytes: ByteArray) { rows[key] = bytes.copyOf() }
    override suspend fun clear() { rows.clear() }
    override suspend fun remove(key: String) { rows.remove(key) }
    override suspend fun cleanup(before: Instant) = Unit
}
class AttachmentRepository(private val call: GeneratedCall, private val cache: ReadCache, private val writes: WriteCoordinator,
                           private val now: () -> Instant = { Instant.fromEpochMilliseconds(0) }) {
    private val store = cache.attachmentStore
    private val thumbnailSlots = Semaphore(2)
    private val downloadSlots = Semaphore(1)
    private val cleanupLock = kotlinx.coroutines.sync.Mutex()
    private var lastCleanup: Instant? = null
    private suspend fun cleanupStore() {
        cleanupLock.lock()
        try {
            val current = now()
            val previous = lastCleanup
            if (previous == null || current < previous || (current - previous).inWholeSeconds >= 3_600) {
                store.cleanup(current - 180.days)
                lastCleanup = current
            }
        } finally { cleanupLock.unlock() }
    }
    private val api = CollaborationApi(call)
    val attachments = cache.view.map { it.attachments.filter { row -> row.deletedAt == null } }.distinctUntilChanged()
    fun forItem(id: String) = attachments.map { it.filter { row -> row.itemId == id } }.distinctUntilChanged()
    internal var afterWrite: suspend () -> Unit = {}
    internal var scopedAfterWrite: (suspend (String) -> Boolean)? = null
    internal var scopedReadsEnabled: () -> Boolean = { false }
    private suspend fun follow(itemId: String) { if (scopedAfterWrite?.invoke(itemId) != true) afterWrite() }
    suspend fun refetchByItemIds(ids: Set<String>) {
        if (!scopedReadsEnabled()) { if (ids.isNotEmpty()) afterWrite() }
        else ids.forEach { follow(it) }
    }
    suspend fun upload(itemId: String, commentId: String?, name: String, mime: String, bytes: ByteArray,
                       key: String = writes.newKey(), progress: (Long, Long) -> Unit = { _, _ -> }): WriteResult<Attachment> = cache.inSession { fence ->
        if (bytes.size > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
        val path = if (commentId == null) "/v1/items/$itemId/attachments" else "/v1/comments/$commentId/attachments"
        val r = writes.execute(key) { call.decode<Attachment>(call.upload(path, name, mime, bytes, it, progress)) }
        if (r !is WriteResult.Success) r else {
            cache.putAttachment(fence, r.value)
            try { follow(itemId); r } catch (e: CancellationException) { throw e } catch (_: Exception) { r.copy(refreshPending = true) }
        }
    }
    suspend fun download(row: Attachment, thumbnail: Boolean = false): ByteArray = cache.inSession { fence ->
        require(row.deletedAt == null && (!thumbnail || row.hasThumbnail))
        val live = cache.view.value.attachments.find { it.id == row.id && it.deletedAt == null && it.version == row.version }
        require(live != null) { "Attachment is no longer visible" }
        call.ensureReady()
        val epoch = cache.sessionEpoch()
        val key = key(row, thumbnail)
        cleanupStore()
        store.read(key)?.let {
            call.ensureReady()
            if (cache.view.value.attachments.none { live -> live.id == row.id && live.version == row.version && live.deletedAt == null }) throw CancellationException("Attachment access changed")
            return@inSession it
        }
        val bytes = try { if (thumbnail) thumbnailSlots.withPermit { call.download(row.id, true) } else downloadSlots.withPermit { call.download(row.id, false) } }
        catch (p: ApiProblem) {
            if (p.deniesVisibility()) {
                try { cache.evictAttachment(row.id) } catch (e: CancellationException) { throw e } catch (e: Exception) { p.addSuppressed(e) }
                // A missing path resource may be just a deleted attachment/comment or absent blob.
                try { ItemsApi(call).itemsGet(row.itemId, app.porchlight.shared.BuildInfo.VERSION).also { cache.putItem(fence, it) } }
                catch (parent: ApiProblem) { cache.evictOnVisibilityFailure(row.itemId, parent) }
                catch (e: CancellationException) { throw e }
                catch (_: Exception) { /* parent visibility is unknown: keep it */ }
            }
            throw p
        }
        if (cache.view.value.attachments.none { it.id == row.id && it.version == row.version && it.deletedAt == null }) throw CancellationException("Attachment access changed")
        // inSession fences the epoch at return; platform stores are cleared on identity change.
        cache.withLiveAttachment(row) {
            if (epoch != cache.sessionEpoch()) throw CancellationException("Household session changed")
            call.ensureReady(); store.write(key, bytes)
        }
        bytes
    }
    /** Compatibility callback for a short handoff; production file I/O uses [openPrepared]. */
    suspend fun open(row: Attachment, viewer: suspend (ByteArray) -> Unit) = cache.inSession { fence ->
        val bytes = download(row)
        cache.withLiveAttachment(row) { call.ensureReady(); viewer(bytes) }
    }
    suspend fun openPrepared(row: Attachment, prepare: suspend (ByteArray) -> PreparedAttachmentViewer) = cache.inSession {
        val bytes = download(row)
        // Drain obsolete ID-based viewer purges before creating a new private file (also on cache hits).
        // The inherited lease is checked again at launch, so eviction during preparation still wins.
        cache.withLiveAttachment(row) { call.ensureReady() }
        val viewer = prepare(bytes)
        var launched = false
        try {
            cache.withLiveAttachment(row) { call.ensureReady(); viewer.launch(); launched = true }
        } finally {
            if (!launched) withContext(NonCancellable) { viewer.discard() }
        }
    }
    /** Desktop originals stay in identity-scoped private files; Android retains its byte-viewer adapter. */
    suspend fun openPreparedContent(row: Attachment, prepare: suspend (AttachmentContent) -> PreparedAttachmentViewer) = cache.inSession { fence ->
        require(row.deletedAt == null && cache.view.value.attachments.any { it.id == row.id && it.version == row.version && it.deletedAt == null })
        call.ensureReady()
        cleanupStore()
        val content = downloadSlots.withPermit {
            store.content(key(row, false)) ?: run {
                val staged = try { store.stage(key(row, false)) { consume -> call.downloadAttachment(row.id, consume) } }
                catch (p: ApiProblem) {
                    if (p.deniesVisibility()) {
                        cache.evictAttachment(row.id)
                        try { ItemsApi(call).itemsGet(row.itemId, app.porchlight.shared.BuildInfo.VERSION).also { cache.putItem(fence, it) } }
                        catch (parent: ApiProblem) { cache.evictOnVisibilityFailure(row.itemId, parent) }
                        catch (e: CancellationException) { throw e }
                        catch (_: Exception) { /* parent visibility remains unknown */ }
                    }
                    throw p
                }
                if (staged == null) null else {
                    var published = false
                    try {
                        cache.withLiveAttachment(row) { call.ensureReady(); staged.publish(); published = true }
                        staged
                    } finally { if (!published) withContext(NonCancellable) { staged.discard() } }
                }
            }
        } ?: ByteAttachmentContent(download(row))
        cache.withLiveAttachment(row) { call.ensureReady() }
        val viewer = prepare(content)
        var launched = false
        try { cache.withLiveAttachment(row) { call.ensureReady(); viewer.launch(); launched = true } }
        finally { if (!launched) withContext(NonCancellable) { viewer.discard() } }
    }
    suspend fun delete(row: Attachment, key: String = writes.newKey()): WriteResult<Unit> = cache.inSession { fence ->
        val r = writes.execute(key) { api.collaborationDeleteAttachment(row.id, row.version.toString(), app.porchlight.shared.BuildInfo.VERSION, it) }
        if (r !is WriteResult.Success) r else {
            cache.evictAttachment(row.id)
            try { follow(row.itemId); r } catch (e: CancellationException) { throw e } catch (_: Exception) { r.copy(refreshPending = true) }
        }
    }
    private fun key(row: Attachment, thumb: Boolean) = attachmentKey(cache.attachmentNamespace, row, thumb)
}
