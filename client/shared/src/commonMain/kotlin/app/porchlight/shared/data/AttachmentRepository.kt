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
/** Keys contain only identity epoch, attachment UUID, version and thumbnail flag; never server-supplied filenames. */
interface AttachmentStore {
    suspend fun read(key: String): ByteArray?
    suspend fun write(key: String, bytes: ByteArray)
    suspend fun clear()
    suspend fun remove(key: String)
    suspend fun removeView(id: String) {}
    suspend fun cleanup(before: Instant)
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
    private val api = CollaborationApi(call)
    val attachments = cache.view.map { it.attachments.filter { row -> row.deletedAt == null } }.distinctUntilChanged()
    fun forItem(id: String) = attachments.map { it.filter { row -> row.itemId == id } }.distinctUntilChanged()
    internal var afterWrite: suspend () -> Unit = {}
    /** C4: metadata has no collection endpoint; item-scoped tickles must resnapshot. */
    suspend fun refetchByItemIds(ids: Set<String>) { if (ids.isNotEmpty()) afterWrite() }
    suspend fun upload(itemId: String, commentId: String?, name: String, mime: String, bytes: ByteArray,
                       key: String = writes.newKey(), progress: (Long, Long) -> Unit = { _, _ -> }): WriteResult<Attachment> = cache.inSession { fence ->
        if (bytes.size > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
        val path = if (commentId == null) "/v1/items/$itemId/attachments" else "/v1/comments/$commentId/attachments"
        val r = writes.execute(key) { call.decode<Attachment>(call.upload(path, name, mime, bytes, it, progress)) }
        if (r !is WriteResult.Success) r else {
            cache.putAttachment(fence, r.value)
            try { afterWrite(); r } catch (e: CancellationException) { throw e } catch (_: Exception) { r.copy(refreshPending = true) }
        }
    }
    suspend fun download(row: Attachment, thumbnail: Boolean = false): ByteArray = cache.inSession { fence ->
        require(row.deletedAt == null && (!thumbnail || row.hasThumbnail))
        val live = cache.view.value.attachments.find { it.id == row.id && it.deletedAt == null && it.version == row.version }
        require(live != null) { "Attachment is no longer visible" }
        call.ensureReady()
        val epoch = cache.sessionEpoch()
        val key = key(row, thumbnail)
        store.cleanup(now() - 180.days)
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
    suspend fun delete(row: Attachment, key: String = writes.newKey()): WriteResult<Unit> = cache.inSession { fence ->
        val r = writes.execute(key) { api.collaborationDeleteAttachment(row.id, row.version.toString(), app.porchlight.shared.BuildInfo.VERSION, it) }
        if (r !is WriteResult.Success) r else {
            cache.evictAttachment(row.id)
            try { afterWrite(); r } catch (e: CancellationException) { throw e } catch (_: Exception) { r.copy(refreshPending = true) }
        }
    }
    private fun key(row: Attachment, thumb: Boolean) = attachmentKey(cache.sessionEpoch(), row, thumb)
}
