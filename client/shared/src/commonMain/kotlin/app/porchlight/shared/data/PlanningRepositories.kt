package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*

fun normalizeAvailabilityTime(value: String): String {
    require(Regex("\\d{2}:\\d{2}(:\\d{2})?").matches(value)) { "Use HH:mm or HH:mm:ss" }
    val parts = value.split(':').map(String::toInt)
    require(parts[0] in 0..23 && parts[1] in 0..59 && (parts.getOrNull(2) ?: 0) in 0..59) { "Invalid local time" }
    return if (parts.size == 2) "$value:00" else value
}
fun validateAvailability(windows: List<Window>) {
    require(windows.size <= 100) { "Too many windows" }
    windows.forEach { require(it.weekday in 0..6); normalizeAvailabilityTime(it.start); normalizeAvailabilityTime(it.end) }
}
fun importSummaryLines(value: ImportSummary): List<String> = listOf(
    "New: ${value.created}; updated: ${value.updated}; unchanged: ${value.unchanged}",
    "Skipped: ${value.skipped}" + (if (value.invalidSkipped != null && value.cancelledSkipped != null)
        " (invalid events: ${value.invalidSkipped}; cancelled/EXDATE occurrences: ${value.cancelledSkipped})" else " (breakdown unavailable)") + "; removed: ${value.removed}; deleted: ${value.deleted}",
) + (if (value.removalSuppressed) listOf("Incomplete calendar: removal suppressed") else emptyList()) + value.warnings.map { "${it.uid ?: "Calendar"} · ${it.code}: ${it.message}" }
private suspend fun <T> fenced(cache: ReadCache?, op: suspend () -> T): T = if (cache == null) op() else cache.inSession { op() }
private suspend fun <T> followUp(result: WriteResult<T>, refresh: suspend () -> Unit): WriteResult<T> {
    if (result !is WriteResult.Success) return result
    return try { refresh(); result } catch (e: CancellationException) { throw e }
    catch (_: Exception) { result.copy(refreshPending = true) }
}
class ImportRepository(call: GeneratedCall, private val writes: WriteCoordinator, private val cache: ReadCache? = null) {
    private val api = CalendarApi(call)
    var afterCommit: suspend () -> Unit = {}
    suspend fun preview(request: ImportRequest): ImportSummary = fenced(cache) {
        api.calendarImport(BuildInfo.VERSION, writes.newKey(), request.copy(preview = true))
    }
    suspend fun commit(request: ImportRequest, key: String = writes.newKey()): WriteResult<ImportSummary> = fenced(cache) {
        followUp(writes.execute(key) { api.calendarImport(BuildInfo.VERSION, it, request.copy(preview = false)) }, afterCommit)
    }
}
class FeedRepository(call: GeneratedCall, private val writes: WriteCoordinator, private val cache: ReadCache? = null) {
    private val api = CalendarApi(call)
    var afterWrite: suspend () -> Unit = {}
    suspend fun list(): List<Source> = fenced(cache) { allPages { api.calendarList(BuildInfo.VERSION, cursor = it, limit = 200).let { p -> p.items to p.nextCursor } } }
    suspend fun add(request: SourceRequest, key: String = writes.newKey()): WriteResult<Source> = fenced(cache) {
        followUp(writes.execute(key) { api.calendarAdd(BuildInfo.VERSION, it, request) }, afterWrite)
    }
    suspend fun update(source: Source, request: SourceRequest, key: String = writes.newKey()): WriteResult<Source> = fenced(cache) {
        followUp(writes.execute(key) { api.calendarUpdate(source.id, source.version.toString(), BuildInfo.VERSION, it, request) }, afterWrite)
    }
    suspend fun refresh(source: Source, key: String = writes.newKey()): WriteResult<Source> = fenced(cache) {
        followUp(writes.execute(key) { api.calendarRefresh(source.id, source.version.toString(), BuildInfo.VERSION, it, buildJsonObject {}) }, afterWrite)
    }
    suspend fun remove(source: Source, key: String = writes.newKey()): WriteResult<Source> = fenced(cache) {
        followUp(writes.execute(key) { api.calendarRemove(source.id, source.version.toString(), BuildInfo.VERSION, it, buildJsonObject {}) }, afterWrite)
    }
}
class AvailabilityRepository(call: GeneratedCall, private val writes: WriteCoordinator, private val cache: ReadCache? = null) {
    private val api = PlanningApi(call)
    var afterWrite: suspend () -> Unit = {}
    val rows: Flow<List<AvailabilityResponse>> = cache?.view?.map { it.availability }?.distinctUntilChanged() ?: flowOf(emptyList())
    fun observe(member: String) = rows.map { it.find { a -> a.memberId == member } }
    suspend fun refresh(member: String): AvailabilityResponse = fenced(cache) {
        api.planningAvailability(member, BuildInfo.VERSION).also { cache?.putAvailability(it) }
    }
    suspend fun put(member: String, version: Int, windows: List<Window>, key: String = writes.newKey()): WriteResult<AvailabilityResponse> = fenced(cache) {
        validateAvailability(windows)
        val request = AvailabilityRequest(windows.map { it.copy(start = normalizeAvailabilityTime(it.start), end = normalizeAvailabilityTime(it.end)) })
        followUp(writes.execute(key) { api.planningPutAvailability(member, version.toString(), BuildInfo.VERSION, it, request) }.also {
            if (it is WriteResult.Success) cache?.putAvailability(it.value)
        }, afterWrite)
    }
}
fun suggestionRejectionKey(row: SuggestionResponse) = "${row.id}:${row.version}:${row.occurrenceVersion}"
class PlanningRepository(call: GeneratedCall, private val writes: WriteCoordinator, private val cache: ReadCache? = null) {
    private val api = PlanningApi(call)
    var afterAccept: suspend () -> Unit = {}
    val rows: Flow<List<SuggestionResponse>> = cache?.view?.map { it.suggestions }?.distinctUntilChanged() ?: flowOf(emptyList())
    suspend fun refresh(member: String? = null): List<SuggestionResponse> = fenced(cache) {
        allPages { api.planningSuggestions(BuildInfo.VERSION, memberId = member, cursor = it, limit = 200).let { p -> p.items to p.nextCursor } }
            .also { cache?.replaceSuggestions(member, it) }
    }
    suspend fun suggest(member: String?, key: String = writes.newKey()): WriteResult<SuggestResponse> = fenced(cache) {
        followUp(writes.execute(key) { api.planningSuggest(BuildInfo.VERSION, it, SuggestRequest(member)) }) { refresh(member); afterAccept() }
    }
    suspend fun accept(row: SuggestionResponse, key: String = writes.newKey()): WriteResult<SuggestionResponse> = fenced(cache) {
        val version = requireNotNull(row.occurrenceVersion) { "Occurrence no longer available; refetch suggestions" }
        refetchRejected(row) { followUp(writes.execute(key) { api.planningAccept(row.id, version.toString(), BuildInfo.VERSION, it, buildJsonObject {}) }.also {
            if (it is WriteResult.Success) cache?.putSuggestion(it.value)
        }, afterAccept) }
    }
    suspend fun dismiss(row: SuggestionResponse, key: String = writes.newKey()): WriteResult<SuggestionResponse> = fenced(cache) {
        refetchRejected(row) { followUp(writes.execute(key) { api.planningDismiss(row.id, row.version.toString(), BuildInfo.VERSION, it, buildJsonObject {}) }.also {
            if (it is WriteResult.Success) cache?.putSuggestion(it.value)
        }, afterAccept) }
    }
    private suspend fun <T> refetchRejected(row: SuggestionResponse, op: suspend () -> T): T {
        try { return op() } catch (p: ApiProblem) {
            if (p.status == 422) {
                // A sealed rejection is not a transport retry. Evict the rejected card if the read cannot complete.
                cache?.replaceSuggestions(row.memberId, cache.view.value.suggestions.filter { it.memberId == row.memberId && it.id != row.id })
                try { refresh(row.memberId) } catch (e: CancellationException) { throw e } catch (_: Exception) { /* explicit UI refetch remains available */ }
            }
            throw p
        }
    }
    suspend fun candidates(item: String): CandidatesResponse = fenced(cache) { api.planningCandidates(item, BuildInfo.VERSION) }
}
class DeviceAdminRepository(call: GeneratedCall, private val writes: WriteCoordinator, private val cache: ReadCache? = null) {
    private val api = DevicesApi(call) // session endpoint returns every member's devices for super admins
    suspend fun list(): List<Device> = fenced(cache) { allPages { api.devicesList(BuildInfo.VERSION, cursor = it, limit = 200).let { p -> p.items to p.nextCursor } } }
    suspend fun revoke(id: String, stepUp: String, key: String = writes.newKey()): WriteResult<RevocationResponse> = fenced(cache) {
        writes.execute(key) { api.devicesRevoke(id, stepUp, BuildInfo.VERSION, it, buildJsonObject {}) }
    }
}
inline fun <reified T> WriteResult<T>.asScreenResult(): WriteResult<JsonElement> = when (this) {
    is WriteResult.Success -> WriteResult.Success(WireJson.encodeToJsonElement(value), refreshPending)
    is WriteResult.VersionConflict -> this
}
