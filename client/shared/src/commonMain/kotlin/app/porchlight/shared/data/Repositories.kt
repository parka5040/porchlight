package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.NoContentSuccess
import app.porchlight.shared.transport.ApiProblem
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import kotlin.time.Instant

internal suspend fun <T> allPages(page: suspend (String?) -> Pair<List<T>, String?>): List<T> {
    val rows = mutableListOf<T>()
    val seen = mutableSetOf<String>()
    var cursor: String? = null
    do {
        val result = page(cursor)
        rows += result.first
        cursor = result.second
        check(cursor == null || seen.add(cursor)) { "Repeated pagination cursor" }
    } while (cursor != null)
    return rows
}

class HouseholdRepository(private val api: MembersApi, private val cache: ReadCache, private val writes: WriteCoordinator,
                          private val version: String = BuildInfo.VERSION) {
    val members = cache.view.map { it.members }.distinctUntilChanged()
    val settings = cache.view.map { it.settings }.distinctUntilChanged()
    suspend fun refreshMembers(): List<MemberRead> = cache.inSession { fence ->
        val mark = cache.receiptMark()
        allPages { cursor -> api.membersList(version, cursor = cursor, limit = 200).let { it.items to it.nextCursor } }
            .also { cache.replaceMembers(fence, it, receivedAfter = mark) }
    }
    suspend fun refreshSettings(): SettingsRead = cache.inSession { fence -> api.membersSettingsGet(version).also { cache.putSettings(fence, it) } }
    suspend fun patchMember(id: String, expectedVersion: Int, patch: MemberPatch, key: String = writes.newKey()): WriteResult<MemberRead> = cache.inSession { fence ->
        val result = writes.execute(key) { api.membersPatch(id, expectedVersion.toString(), version, it, patch) }
        if (result is WriteResult.Success) cache.putMember(fence, result.value)
        return@inSession result // R33: keep pending=true until derived/refetch. Do not immediately overwrite it with false.
    }
    suspend fun putSettings(expectedVersion: Int, settings: TodaySettings, key: String = writes.newKey()): WriteResult<SettingsRead> = cache.inSession { fence ->
        val result = writes.execute(key) { api.membersSettingsPut(expectedVersion.toString(), version, it, settings) }
        if (result is WriteResult.Success) cache.putSettings(fence, result.value)
        return@inSession result
    }
}

class ItemRepository(private val api: ItemsApi, private val cache: ReadCache, private val writes: WriteCoordinator,
                     private val version: String = BuildInfo.VERSION) {
    val items = cache.view.map { it.items }.distinctUntilChanged()
    fun observe(id: String): Flow<ItemRead?> = items.map { rows -> rows.find { it.id == id } }.distinctUntilChanged()
    /** Hooks refetch derived projections only after a sealed successful response has entered the cache. */
    internal var afterWrite: suspend (String?) -> Unit = {}
    suspend fun get(id: String): ItemResponse = cache.inSession { fence ->
        try { api.itemsGet(id, version).also { cache.putItem(fence, it) } }
        catch (p: ApiProblem) { cache.evictOnVisibilityFailure(id, p); throw p }
    }
    suspend fun list(updatedSince: Instant? = null, kind: String? = null): List<ItemResponse> = cache.inSession { fence -> allPages { cursor ->
        api.itemsList(cursor = cursor, limit = 200, updatedSince = updatedSince, kind = kind, includeDeleted = false,
            porchlightClientVersion = version).let { it.items to it.nextCursor }
    }.also { cache.putItems(fence, it) } }
    private suspend fun mutate(key: String, id: String? = null, evictOnNoContent: Boolean = true,
                               operation: suspend (String) -> ItemResponse): WriteResult<ItemResponse?> = cache.inSession { fence ->
        val result = writes.execute(key) { try { operation(it) } catch (_: NoContentSuccess) { null } }
        if (result !is WriteResult.Success) return@inSession result
        val value = result.value
        if (value == null) {
            // A split's 204 refers to its new tail, never the still-visible head. A create may
            // also commit without a representation: reconcile via snapshot without guessing an ID.
            if (evictOnNoContent && id != null) cache.evictItem(id)
            var pending = false
            try { afterWrite(id) }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { pending = true }
            return@inSession result.copy(refreshPending = pending)
        }
        cache.putItem(fence, value)
        var pending = false
        try { if (value.deletedAt == null) afterWrite(value.id) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { pending = true } // Server success stays success; never replay due to a failed refetch.
        return@inSession result.copy(refreshPending = pending)
    }
    suspend fun create(request: CreateRequest, key: String = writes.newKey()) = mutate(key) { api.itemsCreate(version, it, request) }
    suspend fun edit(id: String, expectedVersion: Int, edit: ItemEdit, scope: EditScope = EditScope.All,
                     occurrenceId: String? = null, key: String = writes.newKey()): WriteResult<ItemResponse?> = cache.inSession { fence ->
        require((scope == EditScope.ThisAndFuture) == (occurrenceId != null))
        return@inSession mutate(key, id, evictOnNoContent = scope != EditScope.ThisAndFuture) { api.itemsPatch(id = id, scope = scope.wire, occurrenceId = occurrenceId,
            ifMatch = expectedVersion.toString(), porchlightClientVersion = version, idempotencyKey = it, jsonObject = edit.body) }
    }
    suspend fun delete(id: String, expectedVersion: Int, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsDelete(id, expectedVersion.toString(), version, it, buildJsonObject {})
    }
    suspend fun take(id: String, expectedVersion: Int, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsTake(id, expectedVersion.toString(), version, it, buildJsonObject {})
    }
    suspend fun setAssignees(id: String, expectedVersion: Int, members: List<String>, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsAssignees(id, expectedVersion.toString(), version, it, AssigneesRequest(members))
    }
    suspend fun setViewers(id: String, expectedVersion: Int, members: List<String>, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsViewers(id, expectedVersion.toString(), version, it, ViewersRequest(members))
    }
    suspend fun setReminders(id: String, expectedVersion: Int, offsets: List<Int>, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsReminders(id, expectedVersion.toString(), version, it, RemindersRequest(offsets))
    }
}

class BoardRepository(private val api: ReadsApi, private val cache: ReadCache, private val version: String = BuildInfo.VERSION) {
    val cards = cache.view.map { it.board }.distinctUntilChanged()
    private var currentScope = "household"
    /** The complete board for [scope]; writes nothing (a snapshot stages it, see [SyncController.refresh]). */
    internal suspend fun fetch(scope: String = currentScope): List<ItemRead> {
        require(scope in setOf("mine", "household"))
        return allPages { cursor -> api.readsBoard(version, scope = scope, cursor = cursor, limit = 200).let { it.items to it.nextCursor } }
    }
    /** Replaces the cached board only after every page arrived (one transaction). */
    suspend fun refresh(scope: String = currentScope): List<ItemRead> = cache.inSession { fence ->
        val rows = fetch(scope)
        currentScope = scope
        cache.replaceBoard(fence, rows)
        return@inSession rows
    }
    fun neighbours(mover: String, drop: BoardDrop) = boardNeighbours(cache.view.value.board, mover, drop)
}

class AgendaRepository(private val api: ReadsApi, private val cache: ReadCache, private val version: String = BuildInfo.VERSION) {
    val rows = cache.view.map { it.agenda }.distinctUntilChanged()
    /** The complete agenda of [memberId] (null = own) for [from, to); writes nothing. */
    internal suspend fun fetch(from: Instant, to: Instant, memberId: String? = null): AgendaWindow {
        require(to > from)
        val rows = allPages { cursor -> api.readsAgenda(from, to, version, memberId = memberId, cursor = cursor, limit = 200).let { it.items to it.nextCursor } }
        return AgendaWindow(from, to, memberId, rows)
    }
    /** Replaces, in place, only that member's rows the window covered; other members/windows stay cached. */
    suspend fun refresh(from: Instant, to: Instant, memberId: String? = null): List<ItemRead> = cache.inSession { fence ->
        val window = fetch(from, to, memberId)
        cache.replaceAgenda(fence, window)
        return@inSession window.rows
    }
}

/** One `/v1/search` hit. Items decode to the generated [ItemRead]; other sources (lists, owned by C5) stay raw JSON. */
sealed interface SearchHit {
    data class Item(val item: ItemRead) : SearchHit
    data class Other(val entityType: String, val json: JsonObject) : SearchHit
}
data class SearchResults(val hits: List<SearchHit>, val nextCursor: String?)

class SearchRepository(private val api: ReadsApi, private val version: String = BuildInfo.VERSION) {
    /** Online only, never changes cache visibility or stores a query/queued search. */
    suspend fun search(query: String, cursor: String? = null, limit: Int = 50): SearchResults {
        val page = api.readsSearch(query, version, cursor, limit)
        return SearchResults(page.items.map { hit ->
            when (val type = hit["entity_type"]?.jsonPrimitive?.contentOrNull) {
                "item" -> SearchHit.Item(try { WireJson.decodeFromJsonElement<ItemRead>(hit) }
                    catch (e: CancellationException) { throw e }
                    catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid API response") })
                else -> SearchHit.Other(type ?: "unknown", hit)
            }
        }, page.nextCursor)
    }
}

class OccurrenceRepository(private val api: ItemsApi, private val cache: ReadCache, private val writes: WriteCoordinator,
                           private val board: BoardRepository, private val version: String = BuildInfo.VERSION) {
    val occurrences = cache.view.map { it.occurrences }.distinctUntilChanged()
    fun forItem(id: String) = occurrences.map { rows -> rows.filter { it.itemId == id } }.distinctUntilChanged()
    internal var afterWrite: suspend (String) -> Unit = {}
    suspend fun refresh(itemId: String, from: Instant, to: Instant, includeUndated: Boolean = false): List<OccurrenceResponse> = cache.inSession { fence ->
        val mark = cache.receiptMark()
        allPages { cursor ->
            api.itemsOccurrences(itemId, undated = includeUndated, cursor = cursor, limit = 200, from = from, to = to, porchlightClientVersion = version).let { it.items to it.nextCursor }
        }.also { cache.putOccurrences(fence, itemId, it, from, to, includeUndated, receivedAfter = mark) }
    }
    private suspend fun mutate(key: String, id: String, operation: suspend (String) -> OccurrenceResponse): WriteResult<OccurrenceResponse?> = cache.inSession { fence ->
        val parentId = cache.view.value.occurrences.find { it.id == id }?.itemId
            ?: cache.view.value.board.find { it.occurrence?.id == id }?.id
            ?: cache.view.value.agenda.find { it.occurrence?.id == id }?.id
        val result = writes.execute(key) { try { operation(it) } catch (_: NoContentSuccess) { null } }
        if (result !is WriteResult.Success) return@inSession result
        val value = result.value
        if (value == null) cache.evictOccurrence(id) else cache.putOccurrence(fence, value)
        var pending = false
        try { afterWrite(value?.itemId ?: parentId ?: error("Parent refresh required")) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { pending = true }
        return@inSession result.copy(refreshPending = pending)
    }
    suspend fun patch(id: String, expectedVersion: Int, edit: ItemEdit, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsPatchOccurrence(id, expectedVersion.toString(), version, it, edit.body)
    }
    suspend fun status(id: String, expectedVersion: Int, status: OccurrenceStatus, key: String = writes.newKey()) =
        patch(id, expectedVersion, ItemEdit().text("status", status.value), key)
    suspend fun complete(id: String, expectedVersion: Int, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsComplete(id, expectedVersion.toString(), version, it, buildJsonObject {})
    }
    suspend fun reopen(id: String, expectedVersion: Int, key: String = writes.newKey()) = mutate(key, id) {
        api.itemsReopen(id, expectedVersion.toString(), version, it, buildJsonObject {})
    }
    /** One corrected retry after a sealed stale-neighbour/invalid-reference response, with a NEW key (R38). */
    suspend fun move(id: String, expectedVersion: Int, drop: BoardDrop, key: String = writes.newKey(),
                     visible: (List<ItemRead>) -> List<ItemRead> = { it }, replay: BoardMoveReplay = BoardMoveReplay()): WriteResult<OccurrenceResponse?> = cache.inSession { fence ->
        suspend fun attempt(operationKey: String): WriteResult<OccurrenceResponse?> = cache.inSession { fence ->
            replay.activeKey = operationKey
            val body = replay.bodies.getOrPut(operationKey) {
                val bounds = boardNeighbours(visible(cache.view.value.board), id, drop)
                val move = buildJsonObject {
                    bounds.afterId?.let { put("after_id", it) }
                    bounds.beforeId?.let { put("before_id", it) }
                }
                ItemEdit().text("status", drop.status).set("board_move", move)
            }
            return@inSession patch(id, expectedVersion, body, operationKey)
        }
        try { return@inSession attempt(key) }
        catch (p: ApiProblem) {
            if (!isStaleBoardReference(p)) throw p
            board.refresh()
            return@inSession attempt(writes.newKey())
        }
    }
}
