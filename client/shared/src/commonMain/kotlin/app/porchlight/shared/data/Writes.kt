package app.porchlight.shared.data

import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.delay
import kotlinx.serialization.json.*
import kotlin.uuid.Uuid
import kotlin.uuid.ExperimentalUuidApi

sealed interface WriteResult<out T> {
    data class Success<T>(val value: T, val refreshPending: Boolean = false) : WriteResult<T>
    /** current is kept verbatim for C3's compare view; no implicit overwrite or automatic version retry. */
    data class VersionConflict(val current: JsonElement?, val problem: ApiProblem) : WriteResult<Nothing>
}
class OfflineWrite : Exception("Writes require an online household connection")

/** These responses do not seal the original operation; even exhausted retries must retain its intent/key. */
fun ApiProblem.isUnsealedContention(): Boolean = status == 409 &&
    (type == "idempotency-in-progress" || (type == "conflict" && retryAfterMillis != null))

/**
 * Not a stored server decision, so the outcome is unknown and the operation is NOT sealed: any 5xx (the server rolls
 * back and releases the key; a proxy may have answered after a commit), an error page that is not a problem document
 * (`http-error`), or a reply that could not be decoded (`invalid-response`, including an undecodable 2xx). The caller
 * keeps the frozen key and immutable request; only an identical retry or an explicit abandon follows.
 */
fun ApiProblem.isAmbiguousOutcome(): Boolean = status >= 500 || type == "invalid-response" || type == "http-error"

class WriteCoordinator(
    private val online: () -> Boolean = { true },
    val newKey: () -> String = { randomOperationKey() },
    private val wait: suspend (Long) -> Unit = { delay(it) },
    private val maxAttempts: Int = 3,
) {
    init { require(maxAttempts in 1..5) }
    suspend fun <T> execute(key: String = newKey(), operation: suspend (String) -> T): WriteResult<T> {
        if (!online()) throw OfflineWrite()
        var attempt = 0
        while (true) {
            try { return WriteResult.Success(operation(key)) }
            catch (p: ApiProblem) {
                if (p.status == 409 && p.type == "version-conflict") return WriteResult.VersionConflict(p.extras["current"], p)
                // Only explicit unsealed contention is retryable. Never replay ambiguous IO failures.
                if (!p.isUnsealedContention() || ++attempt >= maxAttempts) throw p
                // The UI must not stay Saving for a proxy/server's day-long Retry-After.
                wait(maxOf(p.retryAfterMillis ?: 1000L, 250L shl (attempt - 1)).coerceAtMost(5000L))
                if (!online()) throw OfflineWrite()
            }
        }
    }
}
@OptIn(ExperimentalUuidApi::class)
fun randomOperationKey(): String = Uuid.random().toString()

/** Presence-aware write document. null means clear, omission means retain; local_* fields stay wall-clock strings. */
data class ItemEdit(val body: JsonObject = buildJsonObject {}) {
    fun set(field: String, value: JsonElement) = copy(body = JsonObject(body + (field to value)))
    fun text(field: String, value: String) = set(field, JsonPrimitive(value))
    fun clear(field: String) = set(field, JsonNull)
    fun local(field: String, value: String) = text(field, value)
}
enum class EditScope(val wire: String) { All("all"), ThisAndFuture("this_and_future") }
/** A drop into the [status] column at [index], counted in that column's visible cards WITHOUT the moving card
 * (0 = top; an index past the end appends). Dropping a card back where it was in its own column keeps its index. */
data class BoardDrop(val status: String, val index: Int)
data class BoardNeighbours(val afterId: String?, val beforeId: String?)
/** UI retains this with an ambiguous move; each key always replays identical neighbours, even after cache updates. */
class BoardMoveReplay {
    internal val bodies = mutableMapOf<String, ItemEdit>()
    var activeKey: String? = null
        internal set
}

/** Input is the ACL-filtered, fully paginated, visible board projection, not all cached occurrences. */
fun boardNeighbours(rows: List<ItemRead>, mover: String, drop: BoardDrop): BoardNeighbours {
    require(drop.index >= 0)
    val cards = rows.mapNotNull { it.occurrence }.filter { it.id != mover && it.status == drop.status }
        .sortedWith(compareBy({ it.boardPosition }, { it.id }))
    val index = drop.index.coerceAtMost(cards.size)
    return BoardNeighbours(cards.getOrNull(index - 1)?.id, cards.getOrNull(index)?.id)
}
fun isStaleBoardReference(p: ApiProblem): Boolean = p.status == 422 && p.extras["errors"]?.jsonArray?.any {
    it.jsonObject["code"]?.jsonPrimitive?.content in setOf("invalid-reference", "stale-neighbour")
} == true
