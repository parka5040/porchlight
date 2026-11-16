package app.porchlight.shared.data

import app.porchlight.shared.api.models.Attachment
import app.porchlight.shared.db.PorchlightDb

internal fun attachmentKey(namespace: Long, row: Attachment, thumbnail: Boolean) =
    "$namespace-${row.id}-${row.version}-${if (thumbnail) "thumb" else "file"}"

/** Cache-owned and serialized with metadata commits and private-file writes. Obligations are
 * inserted inside the metadata transaction; only successful artifact removal retires them. */
internal class AttachmentCleanup(private val store: AttachmentStore, private val now: () -> kotlin.time.Instant,
                                 private val database: () -> PorchlightDb) {
    // Owner/reset recovery also has a durable missing-owner boundary, independent of disposable rows.
    private var clear = false
    fun clearIdentity() { clear = true }
    fun transition(before: CacheView, after: CacheView, namespace: Long, clearIdentity: Boolean): Boolean {
        val q = database().cacheQueries
        if (clearIdentity) {
            q.clearAttachmentCleanup(); q.putAttachmentCleanup("clear", "identity")
            return true
        }
        val live = after.attachments.filter { it.deletedAt == null }.associateBy { it.id }
        val removed = before.attachments.filter { it.deletedAt == null && live[it.id]?.version != it.version }
        removed.forEach { row ->
            q.putAttachmentCleanup("file", attachmentKey(namespace, row, false))
            q.putAttachmentCleanup("file", attachmentKey(namespace, row, true))
            q.putAttachmentCleanup("view", row.id)
        }
        return removed.isNotEmpty()
    }
    suspend fun drain(): SyncState? {
        val db = database()
        val q = db.cacheQueries
        val pending = q.allAttachmentCleanup().executeAsList()
        val previous = q.getCleanupFailure().executeAsOneOrNull()
        if (previous != null && previous.retry_at > now().toEpochMilliseconds())
            throw IllegalStateException("Private file cleanup requires storage recovery")
        try {
            if (clear || pending.any { it.kind == "clear" }) {
                store.clear()
                val promoted = db.transactionWithResult { q.clearAttachmentCleanup(); q.clearCleanupFailure(); promoteCursor(db) }
                clear = false
                return promoted
            }
            var failure: Exception? = null
            for (row in pending) try {
                when (row.kind) {
                    "file" -> store.remove(row.id)
                    "view" -> store.removeView(row.id)
                    else -> error("Invalid attachment cleanup obligation")
                }
                db.transaction { q.deleteAttachmentCleanup(row.kind, row.id) }
            } catch (e: Exception) { if (failure == null) failure = e }
            failure?.let { throw it }
            return db.transactionWithResult { q.clearCleanupFailure(); promoteCursor(db) }
        } catch (e: Exception) {
            val attempts = ((previous?.attempts ?: 0) + 1).coerceAtMost(7)
            val wait = (60_000L shl (attempts.toInt() - 1)).coerceAtMost(3_600_000)
            db.transaction { q.putCleanupFailure(attempts, now().toEpochMilliseconds() + wait) }
            throw e
        }
    }
    private fun promoteCursor(db: PorchlightDb): SyncState? {
        val q = db.cacheQueries
        val pending = q.getPendingSync().executeAsOneOrNull() ?: return null
        q.putSync(pending.cursor, pending.last_full_sync)
        q.clearPendingSync()
        return SyncState(pending.cursor, kotlin.time.Instant.fromEpochMilliseconds(pending.last_full_sync))
    }
}

