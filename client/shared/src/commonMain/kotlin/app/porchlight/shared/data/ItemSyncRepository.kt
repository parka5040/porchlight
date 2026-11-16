package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.ItemSyncApi
import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.ApiProblem
import kotlin.time.Instant

/** A retained agenda scope, not a projection inferred from the rows that happened to survive. */
@kotlinx.serialization.Serializable
data class ItemAgendaWindow(val memberId: String, val from: Instant, val to: Instant)

/** Keep page coverage, including exclusive input cursors, even after merging all returned rows. */
data class ItemBundleCoverage(val pages: List<SyncCoverage>)
data class ItemBundleRows(val pages: List<ItemSyncRead>) {
    val items get() = pages.flatMap { it.items.orEmpty() }.associateBy { it.id }.values.toList()
    val occurrences get() = pages.flatMap { it.occurrences.orEmpty() }.associateBy { it.id }.values.toList()
    val scheduledReminders get() = pages.flatMap { it.scheduledReminders.orEmpty() }.associateBy { it.id }.values.toList()
    val board get() = pages.flatMap { it.board.orEmpty() }.associateBy { it.occurrence?.id }.values.toList()
    val agenda get() = pages.flatMap { it.agenda.orEmpty() }.groupBy { ItemAgendaWindow(it.memberId, it.from, it.to) }.map { (scope, parts) ->
        SyncAgenda(scope.memberId, scope.from, scope.to, parts.flatMap { it.items }.associateBy { it.agenda?.occurrenceId }.values.toList())
    }
    val suggestions get() = pages.flatMap { it.suggestions.orEmpty() }.associateBy { it.id }.values.toList()
    val comments get() = pages.flatMap { it.comments.orEmpty() }.associateBy { it.id }.values.toList()
    val attachments get() = pages.flatMap { it.attachments.orEmpty() }.associateBy { it.id }.values.toList()
    val lists get() = pages.flatMap { it.lists.orEmpty() }.associateBy { it.id to it.occurrenceId }.values.toList()
}
sealed interface ItemSyncResult {
    data class Bundle(val coverage: ItemBundleCoverage, val rows: ItemBundleRows) : ItemSyncResult
    data object Evicted : ItemSyncResult
    /** No coverage was granted. Reconcile with ordinary paginated reads, never an empty replacement. */
    data object Fallback : ItemSyncResult
}

/** Fetch only. The scheduler owns the request-start cache fence, installs and global prefix acknowledgement. */
class ItemSyncRepository(private val call: GeneratedCall, private val supported: () -> Boolean,
                         private val version: String = BuildInfo.VERSION) {
    private val api = ItemSyncApi(call)
    suspend fun fetch(itemId: String, agendaWindows: List<ItemAgendaWindow> = emptyList(),
                      includeCollaboration: Boolean = false): ItemSyncResult = call.withReadSession {
        if (!supported()) return@withReadSession ItemSyncResult.Fallback
        val windows = agendaWindows.distinct()
        require(windows.size <= 8 && windows.all { it.from < it.to })
        try {
            val pages = mutableListOf<ItemSyncRead>()
            val seen = mutableSetOf<Pair<List<String>, String?>>()
            var agendaCursors: List<String> = emptyList()
            var commentsCursor: String? = null
            do {
                val first = pages.isEmpty()
                val page = api.itemSyncGet(itemId, version,
                    agenda = if (first) windows.map { "${it.memberId}:${it.from}:${it.to}" } else null,
                    include = if (first && includeCollaboration) "collaboration" else null,
                    agendaCursor = agendaCursors.takeUnless { first }, commentsCursor = commentsCursor)
                if (!supported()) return@withReadSession ItemSyncResult.Fallback
                require(page.coverage.itemId == itemId && page.coverage.continued != first) { "Invalid item sync coverage" }
                require(page.coverage.agenda.all { it.complete == (it.nextCursor == null) }) { "Invalid agenda continuation" }
                require(!page.coverage.comments || page.coverage.commentsComplete == (page.commentsNextCursor == null)) { "Invalid comment continuation" }
                if (first) {
                    require(page.coverage.itemSections && page.items?.singleOrNull()?.id == itemId)
                    require(page.coverage.agenda.map { ItemAgendaWindow(it.memberId, it.from, it.to) }.toSet() == windows.toSet())
                    require(!includeCollaboration || (page.coverage.collaboration && page.coverage.comments))
                } else {
                    require(!page.coverage.itemSections && !page.coverage.collaboration && !page.coverage.suggestions)
                    require(page.coverage.agenda.mapNotNull { it.cursor }.toSet() == agendaCursors.toSet())
                    require(page.coverage.comments == (commentsCursor != null) && page.coverage.commentsCursor == commentsCursor)
                }
                pages += page
                agendaCursors = page.coverage.agenda.filterNot { it.complete }.map { requireNotNull(it.nextCursor) }
                commentsCursor = page.commentsNextCursor.takeIf { page.coverage.comments && !page.coverage.commentsComplete }
                if (agendaCursors.isNotEmpty() || commentsCursor != null)
                    require(seen.add(agendaCursors to commentsCursor)) { "Non-progressing item sync continuation" }
            } while (agendaCursors.isNotEmpty() || commentsCursor != null)
            ItemSyncResult.Bundle(ItemBundleCoverage(pages.map { it.coverage }), ItemBundleRows(pages))
        } catch (p: ApiProblem) {
            when {
                p.status in setOf(404, 405, 501) && !supported() -> ItemSyncResult.Fallback
                p.status == 404 && p.type == "not-found" -> ItemSyncResult.Evicted
                p.status == 422 && p.type == "item-sync-too-large" -> ItemSyncResult.Fallback
                p.status in setOf(404, 405, 501) -> ItemSyncResult.Fallback
                else -> throw p
            }
        }
    }
}
