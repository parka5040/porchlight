package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlin.time.Instant

internal object SyncBundleFixtures {
    val now = Instant.parse("2026-10-06T12:00:00Z")
    val from = Instant.parse("2026-04-09T12:00:00Z")
    val to = Instant.parse("2028-04-06T12:00:00Z")
    inline fun <reified T> recording(name: String): T = WireJson.decodeFromString(
        checkNotNull(SyncBundleFixtures::class.java.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() })
    val snapshot get() = recording<SnapshotRead>("snapshot")
    val agenda get() = recording<ItemRead>("agenda")
    val board get() = recording<ItemRead>("board")
    fun bundle(snapshot: SnapshotRead = this.snapshot, board: List<ItemRead> = listOf(this.board),
               agenda: List<ItemRead> = listOf(this.agenda), collaboration: Boolean = true): ItemSyncRead {
        val id = snapshot.items.first().id
        val member = snapshot.memberSettings.memberId
        return ItemSyncRead(from, to, SyncCoverage(id, false, true, SyncWindow(from, to), true,
            SyncWindow(from, to), "household", listOf(AgendaCoverage(member, from, to, true)), true,
            collaboration, collaboration, collaboration, member),
            items = snapshot.items.filter { it.id == id }, occurrences = snapshot.occurrences.filter { it.itemId == id },
            scheduledReminders = snapshot.scheduledReminders.filter { it.itemId == id }, board = board,
            agenda = listOf(SyncAgenda(member, from, to, agenda)), suggestions = snapshot.suggestions.filter { it.itemId == id },
            comments = if (collaboration) snapshot.comments.filter { it.itemId == id }.sortedBy { it.id } else emptyList(),
            attachments = if (collaboration) snapshot.attachments.filter { it.itemId == id } else emptyList(),
            lists = if (collaboration) snapshot.lists else emptyList())
    }
    fun continuation(page: ItemSyncRead, agenda: List<SyncAgenda> = emptyList(), agendaCoverage: List<AgendaCoverage> = emptyList(),
                     comments: List<Comment>? = null, commentsCursor: String? = null, commentsNextCursor: String? = null): ItemSyncRead =
        ItemSyncRead(page.windowFrom, page.windowTo, page.coverage.copy(continued = true, itemSections = false,
            suggestions = false, collaboration = false, agenda = agendaCoverage, comments = comments != null,
            commentsCursor = commentsCursor, commentsComplete = comments != null && commentsNextCursor == null),
            agenda = agenda, comments = comments, commentsNextCursor = commentsNextCursor)
}
internal suspend fun ReadCache.installBundle(vararg pages: ItemSyncRead) = inSession { fence ->
    installItemBundle(fence, ItemBundleCoverage(pages.map { it.coverage }), ItemBundleRows(pages.toList()))
}
