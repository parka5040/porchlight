package app.porchlight.shared.screens.collaboration

import androidx.compose.runtime.mutableStateListOf
import app.porchlight.shared.data.randomOperationKey

/**
 * m6c-astra C5-06: hand-off for platform picker/camera results. The picker Activity (or desktop dialog) outlives the
 * composition that launched it: rotation recreates MainActivity, and the PIN gate discards the whole trusted subtree
 * at ON_STOP, which every full-screen picker or camera triggers. So the result is parked here, against the target
 * captured at launch, and is consumed only by the gated composition of that item (never while locked).
 *
 * Owned by [app.porchlight.shared.screens.ScreenSession]: memory only, never persisted, cleared on identity change.
 * Call from the main thread.
 */
class AttachmentPicks {
    /** [commentId] null attaches to the item itself. */
    data class Request(val id: String, val itemId: String, val commentId: String?)
    class Outcome(val request: Request, val picked: PickedAttachment?, val error: String?)

    private val open = mutableMapOf<String, Request>()
    private val ready = mutableStateListOf<Outcome>()

    fun begin(itemId: String, commentId: String?): Request =
        Request(randomOperationKey(), itemId, commentId).also { open[it.id] = it }

    /** Parks a result for its item. False when the request is unknown: completed, or cleared by an identity change. */
    fun complete(id: String, picked: PickedAttachment?, error: String?): Boolean {
        val request = open.remove(id) ?: return false
        if (picked != null || error != null) ready += Outcome(request, picked, error)
        return true
    }

    /** Results waiting for [itemId]'s composition; snapshot state, so a composed reader is notified. */
    fun waiting(itemId: String): List<Outcome> = ready.filter { it.request.itemId == itemId }

    fun take(outcome: Outcome): Boolean = ready.remove(outcome)

    fun clear() { open.clear(); ready.clear() }
}
