package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

/** Retain with an ambiguous reorder. The active key always identifies the actual neighbours sent. */
class ListMoveReplay {
    internal val bodies = mutableMapOf<String, Pair<Int, JsonObject>>()
    var activeKey: String? = null
        internal set
}
val ListResponse.displayName: String get() = if (redacted == true) "Private list" else name.orEmpty()
val ListResponse.cleanupOnly: Boolean get() = redacted == true ||
    (itemId != null && capabilities.toSet() == setOf("view", "delete"))

class ListRepository(private val call: GeneratedCall, private val cache: ReadCache, private val writes: WriteCoordinator,
                     private val version: String = BuildInfo.VERSION) {
    private val api = ListsApi(call)
    val lists = cache.view.map { it.lists }.distinctUntilChanged()
    fun observe(id: String, occurrenceId: String? = null) = lists.map { rows -> rows.find { it.id == id && (it.mode == ListMode.persistent || it.occurrenceId == occurrenceId) } }.distinctUntilChanged()
    internal var afterWrite: suspend () -> Unit = {}
    suspend fun get(id: String, occurrenceId: String? = null): ListResponse = cache.inSession { fence ->
        val mark = cache.receiptMark()
        var requestedScope: String? = null
        try {
            val known = cache.view.value.lists.find { it.id == id }
            val fetched = if (occurrenceId != null && known == null) cache.putList(fence, api.listsGet(id, version, null), mark) else null
            if (fetched != null && (fetched.mode == ListMode.persistent || fetched.cleanupOnly)) return@inSession fetched
            val template = fetched ?: known
            requestedScope = if (template?.mode == ListMode.persistent) null else occurrenceId
            cache.putList(fence, api.listsGet(id, version, requestedScope), mark)
        }
        catch (p: ApiProblem) {
            evictDeniedScope(id, requestedScope, p)
            throw p
        }
    }
    suspend fun refresh(itemId: String? = null, includeOrphaned: Boolean = false): List<ListResponse> = cache.inSession { fence ->
        val mark = cache.receiptMark()
        val rows = allPages { cursor -> api.listsList(version, cursor, 200, itemId, includeOrphaned).let { it.items to it.nextCursor } }
        // Install visibility fences and purge all scopes atomically with the collection.
        cache.replaceLists(fence, itemId, rows, mark)
    }
    /** C4: at most one template per id plus the cache's bounded occurrence scopes. A failed
     * scope never starves other scopes/ids; report the first non-eviction failure after the batch. */
    suspend fun refetchByIds(ids: Set<String>): Unit = cache.inSession { fence ->
        var failure: Exception? = null
        val retained = cache.view.value.lists.groupBy { it.id }
        for (id in ids) {
            val scopes = retained[id].orEmpty().map { it.occurrenceId }.distinct().ifEmpty { listOf(null) }
            for (scope in scopes) {
                // A snapshot/cleanup may have dropped this scope while earlier GETs were in flight.
                if (scope != null && cache.view.value.lists.none { it.id == id && it.occurrenceId == scope }) continue
                try { get(id, scope) }
                catch (e: CancellationException) { throw e }
                catch (e: Exception) {
                    val evicted = e is ApiProblem && (e.deniesVisibility() || (scope != null && e.invalidOccurrenceReference()))
                    if (!evicted && failure == null) failure = e
                    // A hidden/missing whole list needs no more scoped GETs.
                    if (e is ApiProblem && e.deniesVisibility() && cache.view.value.lists.none { it.id == id }) break
                }
            }
        }
        failure?.let { throw it }
    }
    /** Fetch before the snapshot switch: a failed scoped read must not advance its durable cursor. */
    internal suspend fun fetchRetainedScopes(snapshot: SnapshotRead): List<ListResponse> = cache.inSession {
        val occurrences = snapshot.occurrences.mapTo(mutableSetOf()) { it.id }
        val templates = snapshot.lists.filter { it.mode == ListMode.per_occurrence }.mapTo(mutableSetOf()) { it.id }
        val result = mutableListOf<ListResponse>()
        var failure: Exception? = null
        for (row in cache.view.value.lists.filter { it.id in templates && it.occurrenceId != null && it.occurrenceId in occurrences }) {
            try { result += api.listsGet(row.id, version, row.occurrenceId) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                if (e is ApiProblem && (e.deniesVisibility() || e.invalidOccurrenceReference())) evictDeniedScope(row.id, row.occurrenceId, e)
                else if (failure == null) failure = e
            }
        }
        failure?.let { throw it }
        result
    }
    private suspend fun evictDeniedScope(id: String, scope: String?, problem: ApiProblem) {
        if (scope != null && (problem.status == 404 || problem.invalidOccurrenceReference())) {
            cache.evictListScope(id, scope)
            // A scoped 404 alone cannot distinguish an absent occurrence from a hidden list.
            // Nested reads inherit the original request-start fence, including snapshot refetches.
            if (problem.status == 404) try { get(id) }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { /* get evicts a definitively hidden parent; otherwise keep other scopes. */ }
        } else if (problem.deniesVisibility()) cache.evictList(id)
    }
    private fun ApiProblem.invalidOccurrenceReference(): Boolean = status == 422 && type == "validation-failed" &&
        (extras["errors"] as? JsonArray)?.any { error ->
            val field = error as? JsonObject
            (field?.get("field") as? JsonPrimitive)?.contentOrNull == "occurrence_id" &&
                (field["code"] as? JsonPrimitive)?.contentOrNull == "invalid-reference"
        } == true
    private suspend fun <T> follow(fence: ReadCache.Fence, result: WriteResult<T>, listId: String?, occurrence: String? = null): WriteResult<T> {
        if (result !is WriteResult.Success) return result
        (result.value as? EntryResponse)?.let { cache.putEntry(fence, it) }
        return try { afterWrite(); listId?.let { get(it, occurrence) }; result }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { result.copy(refreshPending = true) }
    }
    suspend fun create(body: CreateList, key: String = writes.newKey()): WriteResult<ListResponse> = cache.inSession { fence ->
        val r = writes.execute(key) { api.listsCreate(version, it, body) }
        if (r is WriteResult.Success) cache.putList(fence, r.value)
        follow(fence, r, (r as? WriteResult.Success)?.value?.id)
    }
    suspend fun edit(row: ListResponse, body: PatchList, key: String = writes.newKey()): WriteResult<ListResponse?> = cache.inSession { fence ->
        val r = writes.execute(key) { k ->
            val answer = call.request("PATCH", "/v1/lists/${row.id}", emptyMap(), headers(k, row.version), WireJson.encodeToJsonElement(body).jsonObject)
            if (answer.isEmpty()) null else call.decode<ListResponse>(answer)
        }
        if (r is WriteResult.Success) { r.value?.let { cache.putList(fence, it) } ?: cache.evictList(row.id) }
        follow(fence, r, if ((r as? WriteResult.Success)?.value == null) null else row.id, row.occurrenceId)
    }
    suspend fun delete(row: ListResponse, key: String = writes.newKey()): WriteResult<Unit> = cache.inSession { fence ->
        val r = writes.execute(key) { call.request("DELETE", "/v1/lists/${row.id}", emptyMap(), headers(it, row.version), null); Unit }
        if (r is WriteResult.Success) cache.evictList(row.id)
        follow(fence, r, null)
    }
    suspend fun viewers(row: ListResponse, ids: List<String>, key: String = writes.newKey()): WriteResult<ListResponse?> = cache.inSession { fence ->
        val r = writes.execute(key) { k ->
            val answer = call.request("PUT", "/v1/lists/${row.id}/viewers", emptyMap(), headers(k, row.version), buildJsonObject { put("viewers", JsonArray(ids.map(::JsonPrimitive))) })
            if (answer.isEmpty()) null else call.decode<ListResponse>(answer)
        }
        if (r is WriteResult.Success) { r.value?.let { cache.putList(fence, it) } ?: cache.evictList(row.id) }
        follow(fence, r, if ((r as? WriteResult.Success)?.value == null) null else row.id, row.occurrenceId)
    }
    suspend fun add(row: ListResponse, body: EntryWrite, key: String = writes.newKey()): WriteResult<EntryResponse> = cache.inSession { fence ->
        follow(fence, writes.execute(key) { api.listsAddEntry(row.id, version, it, body) }, row.id, row.occurrenceId)
    }
    /** Presence-aware JSON permits clearing qty/url and both null neighbours (append), unlike generated nullable defaults. */
    suspend fun editEntry(row: EntryResponse, body: JsonObject, occurrenceId: String? = null, key: String = writes.newKey()): WriteResult<EntryResponse> = cache.inSession { fence ->
        follow(fence, writes.execute(key) { call.decode<EntryResponse>(call.request("PATCH", "/v1/list-entries/${row.id}", emptyMap(), headers(it, row.version), body)) }, row.listId, occurrenceId)
    }
    suspend fun archive(row: EntryResponse, occurrenceId: String? = null, key: String = writes.newKey()): WriteResult<EntryResponse> = cache.inSession { fence ->
        follow(fence, writes.execute(key) { api.listsDeleteEntry(row.id, row.version.toString(), version, it) }, row.listId, occurrenceId)
    }
    suspend fun check(id: String, expectedVersion: Int, checked: Boolean, occurrenceId: String? = null, key: String = writes.newKey()): WriteResult<EntryResponse> = cache.inSession { fence ->
        val r = writes.execute(key) { if (checked) api.listsCheck(id, expectedVersion.toString(), version, it, CheckRequest(occurrenceId))
            else api.listsUncheck(id, expectedVersion.toString(), version, it, CheckRequest(occurrenceId)) }
        follow(fence, r, (r as? WriteResult.Success)?.value?.listId, occurrenceId)
    }
    suspend fun clear(row: ListResponse, key: String = writes.newKey()): WriteResult<ListResponse> = cache.inSession { fence ->
        val r = writes.execute(key) { api.listsClear(row.id, row.version.toString(), version, it, CheckRequest(row.occurrenceId)) }
        if (r is WriteResult.Success) cache.putList(fence, r.value)
        follow(fence, r, row.id, row.occurrenceId)
    }
    suspend fun move(list: ListResponse, entry: EntryResponse, index: Int, replay: ListMoveReplay = ListMoveReplay(), key: String = writes.newKey()): WriteResult<EntryResponse> = cache.inSession { fence ->
        fun document(row: ListResponse): Pair<Int, JsonObject> {
            val current = (row.propertyEntries + row.gotIt).find { it.id == entry.id } ?: entry
            val visible = (if (row.mode == ListMode.persistent && current.checkedAt != null) row.gotIt else row.propertyEntries).filter { it.id != entry.id }
            val i = index.coerceIn(0, visible.size)
            return current.version to buildJsonObject { put("after_id", visible.getOrNull(i - 1)?.id?.let(::JsonPrimitive) ?: JsonNull); put("before_id", visible.getOrNull(i)?.id?.let(::JsonPrimitive) ?: JsonNull) }
        }
        var active = key
        var doc = replay.bodies.getOrPut(active) { document(list) }
        replay.activeKey = active
        try { editEntry(entry.copy(version = doc.first), doc.second, list.occurrenceId, active) }
        catch (p: ApiProblem) {
            val stale = isStaleBoardReference(p)
            if (!stale) throw p
            val fresh = get(list.id, list.occurrenceId)
            active = writes.newKey(); doc = document(fresh); replay.bodies[active] = doc; replay.activeKey = active
            editEntry(entry.copy(version = doc.first), doc.second, list.occurrenceId, active)
        }
    }
    private fun headers(key: String, version: Int) = mapOf("Idempotency-Key" to key, "If-Match" to version.toString(), "Porchlight-Client-Version" to this.version)
}

fun canEditComment(comment: Comment, memberId: String?, now: Instant): Boolean = comment.deletedAt == null && memberId != null && comment.authorId == memberId && now <= comment.createdAt + 15.minutes
fun nextCommentEditDeadline(comments: List<Comment>, memberId: String?, now: Instant): Instant? =
    comments.filter { canEditComment(it, memberId, now) }.minOfOrNull { it.createdAt + 15.minutes }
/** Only returned mention IDs are highlighted; unmatched @text is plain text. Longest member name wins. */
fun mentionRanges(comment: Comment, members: List<MemberRead>): List<IntRange> {
    val names = members.filter { it.id in comment.mentions && it.displayName.isNotEmpty() }.map { it.displayName }.sortedByDescending { it.length }
    val body = comment.body
    val result = mutableListOf<IntRange>(); var i = 0
    while (i < body.length) {
        if (body[i] != '@' || (i > 0 && (body[i - 1].isLetterOrDigit() || body[i - 1] == '_'))) { i++; continue }
        val quoted = body.getOrNull(i + 1) == '"'
        val start = i + if (quoted) 2 else 1
        val name = names.find { name ->
            val end = start + name.length
            body.regionMatches(start, name, 0, name.length, ignoreCase = true) &&
                if (quoted) body.getOrNull(end) == '"'
                else body.getOrNull(end)?.let { !it.isLetterOrDigit() && it != '-' && it != '_' } != false
        }
        if (name != null) {
            val end = start + name.length + if (quoted) 1 else 0
            result += i until end; i = end
        } else i++
    }
    return result
}
const val COMMENT_PAGE_LIMIT = 50

class CommentRepository(call: GeneratedCall, private val cache: ReadCache, private val writes: WriteCoordinator,
                        private val version: String = BuildInfo.VERSION) {
    private val api = CollaborationApi(call)
    val comments = cache.view.map { it.comments }.distinctUntilChanged()
    fun forItem(id: String) = comments.map { it.filter { row -> row.itemId == id }.sortedBy { row -> row.createdAt } }.distinctUntilChanged()
    internal var afterWrite: suspend () -> Unit = {}
    /** One server page; never walk the entire history when opening/refetching a thread. A 404/403 forbidden on the
     * comment collection is parent visibility loss (C5-01): the item and its collaboration rows are evicted. */
    suspend fun page(itemId: String, cursor: String? = null): CommentPage = cache.inSession { fence ->
        val page = try { api.collaborationListComments(itemId, version, cursor, COMMENT_PAGE_LIMIT) }
        catch (p: ApiProblem) { cache.evictOnVisibilityFailure(itemId, p); throw p }
        require(page.items.size <= COMMENT_PAGE_LIMIT) { "Oversized comment page" }
        // Only a complete first page can authoritatively replace a thread. Missing rows on a partial
        // page are not deletions; snapshot/event reconciliation owns the rest of the metadata cache.
        cache.putCommentPage(fence, itemId, cursor, page)
        page
    }
    suspend fun window(itemId: String, olderPages: Int) = cache.inSession { fence -> cache.setCommentWindow(fence, itemId, olderPages) }
    suspend fun refresh(itemId: String): List<Comment> = page(itemId).items
    /** Reconcile through the cached UUID horizon, one 50-row page at a time. No single-comment GET exists. */
    suspend fun refetchByItemIds(ids: Set<String>) = cache.inSession { fence ->
        ids.forEach { itemId ->
            val remaining = cache.view.value.comments.filter { it.itemId == itemId }.associateBy { it.id }.toMutableMap()
            var cursor: String? = null
            val seen = mutableSetOf<String>()
            do {
                val page = page(itemId, cursor)
                val returned = page.items.mapTo(mutableSetOf()) { it.id }
                val boundary = page.items.maxOfOrNull { it.id }
                val traversed = remaining.keys.filter { it in returned || page.nextCursor == null || (boundary != null && it <= boundary) }
                traversed.forEach { id ->
                    val captured = remaining.remove(id)
                    // Missing IDs in a traversed range are deletions. Don't remove a newer live receipt.
                    if (id !in returned && cache.view.value.comments.find { it.id == id } == captured) cache.evictComment(id)
                }
                cursor = page.nextCursor
                if (cursor != null) check(seen.add(cursor)) { "Repeated comment cursor" }
            } while (cursor != null && remaining.isNotEmpty())
        }
    }
    private suspend fun follow(fence: ReadCache.Fence, result: WriteResult<Comment>): WriteResult<Comment> {
        if (result !is WriteResult.Success) return result
        cache.putComments(fence, listOf(result.value))
        return try { afterWrite(); result } catch (e: CancellationException) { throw e } catch (_: Exception) { result.copy(refreshPending = true) }
    }
    suspend fun create(itemId: String, body: String, key: String = writes.newKey()): WriteResult<Comment> = cache.inSession { fence -> follow(fence, writes.execute(key) { api.collaborationCreateComment(itemId, version, it, CommentBody(body)) }) }
    suspend fun edit(row: Comment, body: String, key: String = writes.newKey()): WriteResult<Comment> = cache.inSession { fence -> follow(fence, writes.execute(key) { api.collaborationPatchComment(row.id, row.version.toString(), version, it, CommentBody(body)) }) }
    suspend fun delete(row: Comment, key: String = writes.newKey()): WriteResult<Unit> = cache.inSession { fence ->
        val r = writes.execute(key) { api.collaborationDeleteComment(row.id, row.version.toString(), version, it) }
        if (r !is WriteResult.Success) r else try { cache.evictComment(row.id); refresh(row.itemId); afterWrite(); r }
        catch (e: CancellationException) { throw e } catch (_: Exception) { r.copy(refreshPending = true) }
    }
}
