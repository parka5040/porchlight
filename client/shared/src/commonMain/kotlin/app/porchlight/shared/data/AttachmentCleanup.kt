package app.porchlight.shared.data

import app.porchlight.shared.api.models.Attachment

internal fun attachmentKey(epoch: Long, row: Attachment, thumbnail: Boolean) =
    "$epoch-${row.id}-${row.version}-${if (thumbnail) "thumb" else "file"}"

/** Cache-owned, serialized with metadata commits and private-file writes. Failed removals remain retryable;
 * successful ones are removed from the queue, not repeated by repository and lifecycle observers. */
internal class AttachmentCleanup(private val store: AttachmentStore) {
    private val files = linkedSetOf<String>()
    private val views = linkedSetOf<String>()
    private var clear = false
    fun transition(before: CacheView, after: CacheView, epoch: Long, clearIdentity: Boolean): Boolean {
        clear = clear || clearIdentity
        val live = after.attachments.filter { it.deletedAt == null }.associateBy { it.id }
        val removed = before.attachments.filter { it.deletedAt == null && live[it.id]?.version != it.version }
        removed.forEach { row ->
            files += attachmentKey(epoch, row, false); files += attachmentKey(epoch, row, true)
            views += row.id
        }
        // A fresh authorized response may reference bytes whose earlier purge failed. Those bytes
        // are current again; keep ID-based viewer cleanup queued until the next preparation admission.
        live.values.forEach { row ->
            files.remove(attachmentKey(epoch, row, false)); files.remove(attachmentKey(epoch, row, true))
        }
        return clearIdentity || removed.isNotEmpty()
    }
    suspend fun drain() {
        if (clear) {
            store.clear()
            clear = false; files.clear(); views.clear()
            return
        }
        var failure: Exception? = null
        for (key in files.toList()) try { store.remove(key); files.remove(key) }
        catch (e: Exception) { if (failure == null) failure = e }
        for (id in views.toList()) try { store.removeView(id); views.remove(id) }
        catch (e: Exception) { if (failure == null) failure = e }
        failure?.let { throw it }
    }
}
