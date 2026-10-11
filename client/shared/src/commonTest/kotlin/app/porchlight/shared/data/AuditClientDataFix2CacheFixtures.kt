package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import kotlin.time.Instant

// Test-only immediate fixture installs. Nested interleaving tests inherit their request's lease.
// Production callers must explicitly pass the fence captured by inSession before network I/O.
suspend fun ReadCache.install(snapshot: SnapshotRead, at: Instant? = null, projections: Projections? = null) = inSession { fence -> if (at == null) install(fence, snapshot = snapshot, projections = projections ) else install(fence, snapshot = snapshot, at = at, projections = projections ) }

suspend fun ReadCache.installStaged(snapshot: SnapshotRead, at: Instant? = null, projections: Projections? = null,
                                       receivedAfter: Long? = null, requestCursor: Long? = null, authoritative: Boolean = false,
                                       scopedChecks: List<ListResponse>? = null, beforeCommit: () -> Unit = {}, afterStage: () -> Unit = {}) = inSession { fence -> if (at == null) installStaged(fence, snapshot = snapshot, projections = projections, receivedAfter = receivedAfter, requestCursor = requestCursor, authoritative = authoritative, scopedChecks = scopedChecks, beforeCommit = beforeCommit, afterStage = afterStage ) else installStaged(fence, snapshot = snapshot, at = at, projections = projections, receivedAfter = receivedAfter, requestCursor = requestCursor, authoritative = authoritative, scopedChecks = scopedChecks, beforeCommit = beforeCommit, afterStage = afterStage ) }

suspend fun ReadCache.replaceNotifications(rows: List<Notification>) = inSession { fence -> replaceNotifications(fence, rows = rows ) }

suspend fun ReadCache.putNotificationPrefs(row: PrefsRead) = inSession { fence -> putNotificationPrefs(fence, row = row ) }

suspend fun ReadCache.putAvailability(row: AvailabilityResponse) = inSession { fence -> putAvailability(fence, row = row ) }

suspend fun ReadCache.putSuggestion(row: SuggestionResponse) = inSession { fence -> putSuggestion(fence, row = row ) }

suspend fun ReadCache.replaceSuggestions(member: String?, rows: List<SuggestionResponse>) = inSession { fence -> replaceSuggestions(fence, member = member, rows = rows ) }

suspend fun ReadCache.putList(row: ListResponse, requestedAfter: Long? = null) = inSession { fence -> putList(fence, row = row, requestedAfter = requestedAfter ) }

suspend fun ReadCache.replaceLists(itemId: String?, rows: List<ListResponse>, requestedAfter: Long? = null) = inSession { fence -> replaceLists(fence, itemId = itemId, rows = rows, requestedAfter = requestedAfter ) }

suspend fun ReadCache.visibleSearchLists(rows: List<ListResponse>) = inSession { fence -> visibleSearchLists(fence, rows = rows ) }

suspend fun ReadCache.putEntry(row: EntryResponse) = inSession { fence -> putEntry(fence, row = row ) }

suspend fun ReadCache.putComments(rows: List<Comment>) = inSession { fence -> putComments(fence, rows = rows ) }

suspend fun ReadCache.replaceComments(itemId: String, rows: List<Comment>) = inSession { fence -> replaceComments(fence, itemId = itemId, rows = rows ) }

suspend fun ReadCache.putAttachment(row: Attachment) = inSession { fence -> putAttachment(fence, row = row ) }

suspend fun ReadCache.replaceMembers(rows: List<MemberRead>, receivedAfter: Long? = null) = inSession { fence -> replaceMembers(fence, rows = rows, receivedAfter = receivedAfter ) }

suspend fun ReadCache.putMember(row: MemberRead) = inSession { fence -> putMember(fence, row = row ) }

suspend fun ReadCache.putSettings(row: SettingsRead) = inSession { fence -> putSettings(fence, row = row ) }

suspend fun ReadCache.replaceBoard(rows: List<ItemRead>) = inSession { fence -> replaceBoard(fence, rows = rows ) }

suspend fun ReadCache.replaceAgenda(window: AgendaWindow) = inSession { fence -> replaceAgenda(fence, window = window ) }

suspend fun ReadCache.replaceAgenda(rows: List<ItemRead>) = inSession { fence -> replaceAgenda(fence, rows = rows ) }

suspend fun ReadCache.replaceProjections(projections: Projections) = inSession { fence -> replaceProjections(fence, projections = projections ) }

suspend fun ReadCache.putOccurrences(itemId: String, rows: List<OccurrenceResponse>, from: Instant, to: Instant,
                               includeUndated: Boolean = false, receivedAfter: Long? = null) = inSession { fence -> putOccurrences(fence, itemId = itemId, rows = rows, from = from, to = to, includeUndated = includeUndated, receivedAfter = receivedAfter ) }

suspend fun ReadCache.putItem(response: ItemResponse) = inSession { fence -> putItem(fence, response = response ) }

suspend fun ReadCache.putItems(responses: List<ItemResponse>) = inSession { fence -> putItems(fence, responses = responses ) }

suspend fun ReadCache.putOccurrence(response: OccurrenceResponse) = inSession { fence -> putOccurrence(fence, response = response ) }
