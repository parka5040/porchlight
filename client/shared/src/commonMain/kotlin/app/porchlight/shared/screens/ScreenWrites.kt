package app.porchlight.shared.screens

import app.porchlight.shared.data.*
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.*

data class WriteFeedback(val busy: Boolean = false, val error: String? = null, val current: JsonElement? = null,
    val conflict: Boolean = false, val warning: String? = null, val fieldErrors: Map<String, String> = emptyMap(),
    val retryKey: String? = null, val refreshPending: Boolean = false, val saved: Boolean = false, val savedValue: JsonElement? = null)
/** Owns an immutable request closure and key. Never automatically replays an ambiguous transport result or a 409. */
class ScreenWrites(private val newKey: () -> String = { randomOperationKey() }) {
    private val mutable = MutableStateFlow(WriteFeedback())
    val state = mutable.asStateFlow()
    private var cache: ReadCache? = null
    private var visibilityRevision: Long? = null
    fun bindCache(value: ReadCache) { cache = value; visible(value.view.value) }
    fun visible(view: CacheView?): WriteFeedback {
        if (view != null && visibilityRevision != null && view.visibilityRevision != visibilityRevision) {
            // Keep outcome/retry metadata, never the revoked representation or values derived from it.
            mutable.value = mutable.value.copy(current = null, savedValue = null, warning = null, fieldErrors = emptyMap())
        }
        return mutable.value
    }
    private var pending: (suspend (String) -> WriteResult<JsonElement>)? = null
    private var key: String? = null
    private var actualKey: () -> String? = { null }
    suspend fun submit(activeKey: () -> String? = { null }, operation: suspend (String) -> WriteResult<JsonElement>) {
        if (mutable.value.busy || pending != null) {
            mutable.value = mutable.value.copy(error = "Finish or abandon the pending request before another action.")
            return
        }
        actualKey = activeKey
        visibilityRevision = cache?.view?.value?.visibilityRevision
        pending = operation; key = newKey(); execute()
    }
    suspend fun retry() { if (pending != null && !mutable.value.busy) execute() }
    fun discard() { if (!mutable.value.busy) { pending = null; key = null; mutable.value = WriteFeedback() } }
    private suspend fun execute() {
        val op = pending ?: return
        val k = key ?: return
        mutable.value = WriteFeedback(busy = true)
        try {
            mutable.value = when (val result = op(k)) {
                is WriteResult.VersionConflict -> WriteFeedback(current = result.current, conflict = true, error = "This item changed. Compare yours with current before reapplying.")
                is WriteResult.Success -> {
                    val conflicts = (result.value as? JsonObject)?.get("conflicts") as? JsonArray
                    val warning = conflicts?.takeIf { it.isNotEmpty() }?.joinToString("\n") { c ->
                        val obj = c as? JsonObject
                        "Conflict warning: " + (obj?.get("other_title") as? JsonPrimitive)?.content.orEmpty().ifEmpty { "Busy" }
                    }
                    WriteFeedback(warning = warning, refreshPending = result.refreshPending, saved = true, savedValue = result.value)
                }
            }
            pending = null; key = null
        } catch (e: CancellationException) {
            key = actualKey() ?: k
            mutable.value = WriteFeedback(error = "Request interrupted; outcome unconfirmed. Retry only the identical request.", retryKey = key)
            throw e
        }
        catch (e: ApiProblem) {
            val fields = (e.extras["errors"] as? JsonArray)?.mapNotNull { error ->
                val obj = error as? JsonObject ?: return@mapNotNull null
                (obj["field"] as? JsonPrimitive)?.content?.let { it to ((obj["message"] ?: obj["code"]) as? JsonPrimitive)?.content.orEmpty() }
            }?.toMap().orEmpty()
            if (e.maintenanceMessage != null) {
                key = actualKey() ?: k
                mutable.value = WriteFeedback(error = e.maintenanceMessage, retryKey = key)
            } else if (e.isUnsealedContention()) {
                key = actualKey() ?: k
                mutable.value = WriteFeedback(error = "Request still pending. Retry only the identical request after waiting.", retryKey = key)
            } else if (e.isAmbiguousOutcome()) {
                // A 5xx, proxy page or unreadable reply is not a server decision: the write may have committed.
                key = actualKey() ?: k
                mutable.value = WriteFeedback(error = e.attachmentUploadMessage() ?: "Could not confirm the write. Retry sends the identical request; no offline queue.", retryKey = key)
            } else {
                mutable.value = WriteFeedback(error = "Write rejected (${e.status}). Correct the fields below or refetch.", fieldErrors = fields)
                pending = null; key = null // sealed business error; correction is a new write
            }
        } catch (e: IllegalArgumentException) {
            mutable.value = WriteFeedback(error = "Invalid request. Correct the draft before saving.")
            pending = null; key = null
        } catch (e: Exception) {
            key = (e as? HouseholdLockedAfterReply)?.idempotencyKey ?: actualKey() ?: k
            mutable.value = WriteFeedback(error = if (e is HouseholdLocked) "Unlock Porchlight, then retry the same request." else "Could not confirm the write. Retry sends the identical request; no offline queue.", retryKey = key)
        } finally { visible(cache?.view?.value) }
    }
}
