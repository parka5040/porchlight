package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.transport.WireJson
import app.cash.sqldelight.db.SqlDriver
import kotlinx.coroutines.*
import kotlin.coroutines.AbstractCoroutineContextElement
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.datetime.LocalDate
import kotlinx.datetime.LocalDateTime
import kotlinx.datetime.TimeZone
import kotlinx.datetime.atStartOfDayIn
import kotlinx.datetime.plus
import kotlinx.datetime.toInstant
import kotlinx.datetime.DatePeriod
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import kotlin.time.Duration
import kotlin.time.Duration.Companion.days
import kotlin.time.Instant

/** All projection payloads retain capabilities, pending flags, relationships and reminders verbatim. */
data class SyncState(val cursor: Long, val lastFullSync: Instant)
data class CacheView(
    val members: List<MemberRead> = emptyList(), val items: List<ItemRead> = emptyList(),
    val occurrences: List<OccurrenceRead> = emptyList(), val agenda: List<ItemRead> = emptyList(),
    val board: List<ItemRead> = emptyList(), val settings: SettingsRead? = null,
    val reminders: List<ScheduledReminderRead> = emptyList(), val itemWrites: Map<String, ItemResponse> = emptyMap(), val sync: SyncState? = null,
    val notifications: List<Notification> = emptyList(), val notificationPrefs: PrefsRead? = null,
    val availability: List<AvailabilityResponse> = emptyList(), val suggestions: List<SuggestionResponse> = emptyList(),
    val lists: List<ListResponse> = emptyList(), val comments: List<Comment> = emptyList(), val attachments: List<Attachment> = emptyList(),
    val evictedListIds: Set<String> = emptySet(), val evictedItemIds: Set<String> = emptySet(),
)
/** One fully paginated `/v1/agenda` answer: the server authoritatively covered [from, to) for [memberId]
 * (null = the caller's own member). */
data class AgendaWindow(val from: Instant, val to: Instant, val memberId: String?, val rows: List<ItemRead>)
/** Board (one scope) and agenda (one window), both fetched completely before either replaces cached rows (R59). */
data class Projections(val board: List<ItemRead>, val agenda: AgendaWindow)

/** Item responses keep explicit nulls/defaults when merged over a cached row. */
private val responseJson = Json(WireJson) { encodeDefaults = true }

/** One driver per application. Transactions publish one immutable view, never partially loaded projections. */
class ReadCache(private val open: () -> SqlDriver, private val now: () -> Instant) {
    /** An already opened driver (tests, Android: SQLDelight's Android driver opens lazily itself). */
    constructor(driver: SqlDriver, now: () -> Instant) : this({ driver }, now)
    private var driver: SqlDriver? = null
    // Opened on first use, which is always inside initialize()/transaction() on Dispatchers.Default: desktop file
    // bootstrap (mkdirs, PRAGMA, recreate) never runs on the composition thread.
    private val db by lazy { PorchlightDb(open().also { driver = it }) }
    private val q get() = db.cacheQueries
    private val mutex = Mutex()
    private var closed = false
    private val mutable = MutableStateFlow(CacheView())
    private val mutableSync = MutableStateFlow<SyncState?>(null)
    // N-1/O-c/O-g: occurrence, member and settings responses and item deletions this client received, numbered in
    // receipt order. A snapshot requested before a receipt may predate it; installStaged re-applies the newer ones.
    // Per id the highest version wins, stamped with the latest receipt number. Guarded by [mutex].
    private var receiptSeq = 0L
    private val availabilityReceipts = mutableMapOf<String, Pair<Long, AvailabilityResponse>>()
    private val suggestionReceipts = mutableMapOf<String, Pair<Long, SuggestionResponse>>()
    private val itemReceipts = mutableMapOf<String, Pair<Long, ItemResponse>>()
    private val occurrenceReceipts = mutableMapOf<String, Pair<Long, OccurrenceResponse>>()
    private val occurrenceObservations = mutableMapOf<String, Long>()
    private val visibilityDeletions = mutableMapOf<String, Long>()
    internal var removeEvictedItemFiles: suspend (List<Attachment>) -> Unit = {}
    private val itemDeletions = mutableMapOf<String, Pair<Long, Int>>() // item id -> (receipt, deleted version)
    private val memberReceipts = mutableMapOf<String, Pair<Long, MemberRead>>()
    private val entryReceipts = mutableMapOf<String, Pair<Long, EntryResponse>>()
    private val listReceipts = mutableMapOf<String, Pair<Long, ListResponse>>()
    private val commentReceipts = mutableMapOf<String, Pair<Long, Comment>>()
    private val attachmentReceipts = mutableMapOf<String, Pair<Long, Attachment>>()
    private val collaborationDeletions = mutableMapOf<Pair<String, String>, Long>()
    private var settingsReceipt: Pair<Long, SettingsRead>? = null
    var sessionEpoch: () -> Long = { 0L }
    internal var sessionCommitMutex: Mutex? = null
    private class Lease(val epoch: Long) : AbstractCoroutineContextElement(Key) {
        companion object Key : CoroutineContext.Key<Lease>
    }
    suspend fun <T> inSession(block: suspend () -> T): T {
        val lease = currentCoroutineContext()[Lease] ?: Lease(sessionEpoch())
        return withContext(lease) { block() }
    }
    // Fix 1 used the same schema but retained unbounded scopes: prune that disk history before publishing.
    suspend fun initialize() = transaction { trimListScopes() }
    fun itemWrite(id: String): ItemResponse? = view.value.itemWrites[id]
    val view: StateFlow<CacheView> = mutable.asStateFlow()
    val sync: StateFlow<SyncState?> = mutableSync.asStateFlow()
    private fun publish(next: CacheView, resetVisibility: Boolean = false) {
        val retained = next.lists.mapTo(mutableSetOf()) { it.id }
        val removed = mutable.value.lists.map { it.id }.filter { it !in retained }
        val evicted = if (resetVisibility) emptySet() else (mutable.value.evictedListIds + removed) - retained
        val retainedItems = (next.items + next.board + next.agenda).mapTo(mutableSetOf()) { it.id }
        val removedItems = (mutable.value.items + mutable.value.board + mutable.value.agenda).map { it.id }.filter { it !in retainedItems }
        val evictedItems = if (resetVisibility) emptySet() else (mutable.value.evictedItemIds + removedItems) - retainedItems
        mutable.value = next.copy(evictedListIds = evicted, evictedItemIds = evictedItems); mutableSync.value = next.sync
    }
    private fun load(): CacheView {
        val planning = q.allPlanning().executeAsList()
        return CacheView(
        q.allMembers().executeAsList().map { WireJson.decodeFromString<MemberRead>(it.payload) },
        q.allItems().executeAsList().map { WireJson.decodeFromString<ItemRead>(it.payload) },
        q.allOccurrences().executeAsList().map { WireJson.decodeFromString<OccurrenceRead>(it.payload) },
        q.allAgendas().executeAsList().map { WireJson.decodeFromString<ItemRead>(it.payload) },
        q.allBoards().executeAsList().map { WireJson.decodeFromString<ItemRead>(it.payload) }.sortedWith(compareBy({ it.occurrence?.status }, { it.occurrence?.boardPosition }, { it.occurrence?.id })),
        q.allSettingss().executeAsList().firstOrNull()?.let { WireJson.decodeFromString<SettingsRead>(it.payload) },
        q.allReminders().executeAsList().map { WireJson.decodeFromString<ScheduledReminderRead>(it.payload) },
        q.allWrites().executeAsList().mapNotNull {
            // Caches written before fix round 1 also hold occurrence responses here; only item responses are typed.
            val raw = WireJson.parseToJsonElement(it.payload).jsonObject
            if ("reminder_offsets" in raw) it.id to WireJson.decodeFromJsonElement<ItemResponse>(raw) else null
        }.toMap(),
        q.getSync().executeAsOneOrNull()?.let { SyncState(it.cursor, Instant.fromEpochMilliseconds(it.last_full_sync)) },
        q.allNotifications().executeAsList().map { WireJson.decodeFromString<Notification>(it.payload) },
        q.allNotificationPrefss().executeAsList().firstOrNull()?.let { WireJson.decodeFromString<PrefsRead>(it.payload) },
        planning.filter { it.kind == "availability" }.map { WireJson.decodeFromString<AvailabilityResponse>(it.payload) },
        planning.filter { it.kind == "suggestion" }.map { WireJson.decodeFromString<SuggestionResponse>(it.payload) },
        q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) },
        q.allComments().executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) },
        q.allAttachments().executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) },
        )
    }
    /** [fenced] = false: the caller's inherited session lease does not apply (identity clears must always land). */
    private suspend fun transaction(fenced: Boolean = true, block: () -> Unit) = withContext(Dispatchers.Default) { mutex.withLock {
        val context = currentCoroutineContext()
        val lease = if (fenced) context[Lease] else null
        fun checkLease() {
            if (closed) throw CancellationException("Read cache closed")
            context.ensureActive()
            if (lease != null && lease.epoch != sessionEpoch()) throw CancellationException("Household session changed")
        }
        val fence = sessionCommitMutex
        fence?.lock()
        try {
            checkLease()
            db.transaction { block(); checkLease() }
            publish(load(), resetVisibility = !fenced)
        } finally { fence?.unlock() }
    } }
    private inline fun <reified T> stage(kind: String, id: String, itemId: String?, expiry: Long?, value: T) =
        q.stage(kind, id, itemId, expiry, WireJson.encodeToString(value))

    /** Authoritative switch (R59): entities, cursor and — when given — the board/agenda [projections] fetched for this
     * snapshot commit in ONE transaction. Projection rows of items that are no longer visible disappear (snapshot
     * difference); fetched projections replace theirs in place. Throwing anywhere rolls back every table and cursor. */
    suspend fun install(snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null) =
        installStaged(snapshot, at, projections)
    /** Marks the responses received so far; take it BEFORE requesting a snapshot and pass it to [installStaged]. */
    internal suspend fun receiptMark(): Long = withContext(Dispatchers.Default) { mutex.withLock { receiptSeq } }
    /** The receipt mark plus the cursor installed at the same moment, both taken BEFORE requesting a snapshot (N-1). */
    internal data class SnapshotMark(val receipts: Long, val cursor: Long?)
    internal suspend fun snapshotMark(): SnapshotMark =
        withContext(Dispatchers.Default) { mutex.withLock { SnapshotMark(receiptSeq, mutable.value.sync?.cursor) } }
    /** [receivedAfter]: responses received after this mark may be newer than [snapshot]; inside the same transaction,
     * each one whose version is higher than the snapshot's row replaces it (undated occurrences included, which no
     * windowed refetch can return). [requestCursor]: the cursor installed when [snapshot] was requested; a snapshot
     * older than it is a restore/rollback. Test-only fault injection: [afterStage] runs after every staged row,
     * [beforeCommit] after promotion. */
    internal suspend fun installStaged(snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null,
                                       receivedAfter: Long? = null, requestCursor: Long? = null, authoritative: Boolean = false,
                                       beforeCommit: () -> Unit = {}, afterStage: () -> Unit = {}) = transaction {
        // R81: snapshots replace even lower versions. Only post-request receipts may be re-applied;
        // a cursor regression means those receipts belong to the pre-restore epoch. N-1: compare with the cursor at
        // REQUEST time, never the live one: C4 stream acks advance it while the snapshot loads, and the snapshot's
        // cursor (the server's max stream seq when it was read) is never below a cursor acked before the request.
        val regressed = requestCursor?.let { snapshot.cursor < it } == true
        val items = snapshot.items
        val members = snapshot.members
        val occurrences = snapshot.occurrences
        val settings = snapshot.memberSettings
        val itemsById = items.associateBy { it.id }
        val scopedLists = q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.filter { it.occurrenceId != null }
        val ownZone = zoneOf(members, settings.memberId)
        q.clearStaging()
        members.forEach { stage("Member", it.id, null, null, it); afterStage() }
        items.forEach { stage("Item", it.id, it.id, null, it); afterStage() }
        occurrences.forEach { stage("Occurrence", it.id, it.itemId, expiry(it, ownZone, itemsById[it.itemId]), it); afterStage() }
        snapshot.scheduledReminders.forEach { stage("Reminder", it.id, it.itemId, it.fireAt.toEpochMilliseconds(), it); afterStage() }
        snapshot.lists.forEach { row ->
            stage("List", row.id, row.itemId, null, row); afterStage()
            (row.propertyEntries + row.gotIt).forEach { stage("Entry", it.id, row.id, null, it); afterStage() }
        }
        snapshot.comments.forEach { stage("Comment", it.id, it.itemId, null, it); afterStage() }
        snapshot.attachments.forEach { stage("Attachment", it.id, it.itemId, null, it); afterStage() }
        stage("Settings", settings.memberId, null, null, settings)
        afterStage()
        snapshot.notifications.forEach { stage("Notification", it.id, it.itemId, null, it); afterStage() }
        stage("NotificationPrefs", snapshot.notificationPrefs.memberId, null, null, snapshot.notificationPrefs)
        clearEntities()
        q.promoteNotifications(); q.promoteNotificationPrefss()
        q.promoteMembers(); q.promoteItems(); q.promoteOccurrences(); q.promoteSettingss(); q.promoteReminders()
        q.clearPlanning()
        snapshot.availability.forEach { q.putPlanning("availability", it.memberId, null, WireJson.encodeToString(it)) }
        snapshot.suggestions.forEach { q.putPlanning("suggestion", it.id, it.itemId, WireJson.encodeToString(it)) }
        q.promoteLists(); q.promoteEntrys(); q.promoteComments(); q.promoteAttachments()
        // Snapshot lists are templates, not occurrence check projections. Retain only scopes
        // within this authoritative occurrence window, merging structure/ACLs and independent checks.
        val visibleOccurrences = snapshot.occurrences.mapTo(mutableSetOf()) { it.id }
        val templates = snapshot.lists.associateBy { it.id }
        scopedLists.filter { it.occurrenceId in visibleOccurrences }.forEach { scoped ->
            val template = templates[scoped.id]?.takeIf { it.mode == ListMode.per_occurrence }
            if (template != null && (template.itemId == null || template.itemId in itemsById)) {
                val checks = scoped.propertyEntries.associateBy { it.id }
                putListRow(template.copy(occurrenceId = scoped.occurrenceId, propertyEntries = template.propertyEntries.map { entry ->
                    val old = checks[entry.id]
                    entry.copy(occurrenceId = scoped.occurrenceId, checkedAt = old?.checkedAt, checkedBy = old?.checkedBy)
                }, gotIt = emptyList()))
            }
        }
        occurrenceObservations.clear()
        val installedAt = ++receiptSeq
        occurrences.forEach { occurrenceObservations[it.id] = installedAt }
        q.putSync(snapshot.cursor, at.toEpochMilliseconds())
        q.evictOrphanBoards(); q.evictOrphanAgendas()
        projections?.let {
            replaceBoardRows(it.board)
            replaceAgendaRows(it.agenda, snapshot.members, snapshot.memberSettings.memberId)
        }
        if (!regressed && !authoritative) receivedAfter?.let { reapplyNewerReceipts(snapshot, it) }
        // Every later snapshot is requested after these receipts, so none is needed again.
        clearReceipts()
        cleanRows(at)
        q.clearStaging()
        beforeCommit()
    }
    /** Re-applies only over rows the snapshot holds (M-1): the snapshot carries every undated occurrence of a visible
     * item, so a row it lacks was deleted or reparented on the server and must not come back. Dated rows created
     * after the snapshot come back through the post-install repair refetch. */
    private fun reapplyNewerReceipts(snapshot: SnapshotRead, mark: Long) {
        for ((seq, row) in availabilityReceipts.values) {
            val installed = snapshot.availability.find { it.memberId == row.memberId } ?: continue
            if (seq > mark && installed.version < row.version) q.putPlanning("availability", row.memberId, null, WireJson.encodeToString(row))
        }
        for ((seq, row) in suggestionReceipts.values) {
            val installed = snapshot.suggestions.find { it.id == row.id } ?: continue
            if (seq > mark && installed.version < row.version && snapshot.items.any { it.id == row.itemId })
                q.putPlanning("suggestion", row.id, row.itemId, WireJson.encodeToString(row))
        }
        val items = snapshot.items.associateByTo(mutableMapOf()) { it.id }
        visibilityDeletions.filterValues { it > mark }.keys.forEach { evictItemRows(it); items.remove(it) }
        for ((seq, response) in itemReceipts.values) {
            val installed = items[response.id] ?: continue
            if (seq > mark && installed.version < response.version) {
                putItemRow(response, installed)
                q.getItem(response.id).executeAsOneOrNull()?.let { items[response.id] = WireJson.decodeFromString(it.payload) }
            }
        }
        val occurrences = snapshot.occurrences.associateBy { it.id }
        val zone = zoneOf(snapshot.members, snapshot.memberSettings.memberId)
        for ((id, receipt) in itemDeletions) {
            val (seq, version) = receipt
            val installed = items[id] ?: continue
            if (seq > mark && installed.version < version) { evictItemRows(id); items.remove(id) } // O-g
        }
        for ((seq, response) in occurrenceReceipts.values) {
            if (seq <= mark || response.itemId !in items) continue // older, or the item is no longer visible
            val installed = occurrences[response.id] ?: continue
            if (installed.version < response.version) putOccurrenceRow(response, items, occurrences, zone)
        }
        val members = snapshot.members.associateBy { it.id }
        for ((seq, row) in memberReceipts.values) {
            val installed = members[row.id] ?: continue
            if (seq > mark && installed.version < row.version) q.putMember(row.id, null, null, WireJson.encodeToString(row))
        }
        val lists = snapshot.lists.associateBy { it.id }
        for ((seq, row) in listReceipts.values.toList()) {
            val installed = lists[row.id] ?: continue
            if (seq > mark && (row.occurrenceId == null || row.occurrenceId in occurrences) &&
                (row.version > installed.version || (row.occurrenceId != null && row.version == installed.version)) &&
                (row.itemId == null || row.itemId in items)) putListRow(row)
        }
        for ((seq, row) in entryReceipts.values.toList()) {
            val parent = lists[row.listId] ?: continue
            val installed = (parent.propertyEntries + parent.gotIt).find { it.id == row.id } ?: continue
            if (seq > mark && row.version > installed.version && (parent.itemId == null || parent.itemId in items)) putEntryRow(row)
        }
        val comments = snapshot.comments.associateBy { it.id }
        for ((seq, row) in commentReceipts.values) {
            val installed = comments[row.id] ?: continue
            if (seq > mark && row.version > installed.version && row.itemId in items) q.putComment(row.id, row.itemId, null, WireJson.encodeToString(row))
        }
        val attachments = snapshot.attachments.associateBy { it.id }
        for ((seq, row) in attachmentReceipts.values) {
            val installed = attachments[row.id] ?: continue
            if (seq > mark && row.version > installed.version && row.itemId in items) q.putAttachment(row.id, row.itemId, null, WireJson.encodeToString(row))
        }
        for ((resource, seq) in collaborationDeletions) if (seq > mark) when (resource.first) {
            "list" -> { q.deleteList(resource.second); q.evictEntry(resource.second) }
            "comment" -> { q.deleteComment(resource.second); evictCommentAttachments(resource.second) }
            "attachment" -> q.deleteAttachment(resource.second)
        }
        settingsReceipt?.let { (seq, row) ->
            val installed = snapshot.memberSettings
            if (seq > mark && installed.memberId == row.memberId && installed.version < row.version) {
                q.clearSettingss(); q.putSettings(row.memberId, null, null, WireJson.encodeToString(row))
            }
        }
    }
    private fun zoneOf(members: List<MemberRead>, memberId: String?): TimeZone =
        members.find { it.id == memberId }?.tzid?.let { runCatching { TimeZone.of(it) }.getOrNull() } ?: TimeZone.UTC
    private fun ownZone() = mutable.value.let { zoneOf(it.members, it.settings?.memberId) }
    /** A date-only row stays relevant for its entire member-local day (occurrences and agenda alike). */
    private fun endOfDay(date: LocalDate, zone: TimeZone) = (date + DatePeriod(days = 1)).atStartOfDayIn(zone)
    private fun expiry(row: OccurrenceRead, zone: TimeZone, item: ItemRead? = null): Long? {
        val effective = row.effective(item, zone)
        return (effective.endsAt ?: effective.dueAt ?: effective.startsAt
            ?: (effective.localEnd ?: effective.localDue ?: effective.localStart)?.let { resolveOccurrenceLocal(it, zone) }
            ?: effective.dueDate?.let { endOfDay(it, zone) } ?: row.closedAt ?: row.completedAt)?.toEpochMilliseconds()
    }
    private fun agendaExpiry(row: ItemRead, members: List<MemberRead>): Long? = row.agenda?.let { agenda ->
        agenda.endsAt ?: agenda.dueAt ?: agenda.startsAt ?: agenda.dueDate?.let { endOfDay(it, zoneOf(members, agenda.memberId)) }
    }?.toEpochMilliseconds()
    private fun clearEntities() {
        q.clearMembers(); q.clearItems(); q.clearOccurrences(); q.clearSettingss(); q.clearReminders(); q.clearWrites(); q.clearNotifications(); q.clearNotificationPrefss(); q.clearLists(); q.clearEntrys(); q.clearComments(); q.clearAttachments()
    }
    private fun cleanRows(at: Instant) {
        val cutoff = (at - 180.days).toEpochMilliseconds()
        q.cleanOccurrences(cutoff); q.cleanAgendas(cutoff); q.cleanReminders(cutoff)
        val retainedOccurrences = q.allOccurrences().executeAsList().mapTo(mutableSetOf()) { it.id }
        occurrenceObservations.keys.retainAll(retainedOccurrences)
        scopedListRows().filter { it.occurrence_id !in retainedOccurrences }.forEach { evictListScopeRows(it.id, it.occurrence_id) }
        trimListScopes()
    }
    /** The board projection is one scope, fully paginated: replaced as a whole inside the caller's transaction. */
    private fun replaceBoardRows(rows: List<ItemRead>) {
        q.clearBoards()
        rows.forEach { row -> row.occurrence?.let { q.putBoard(it.id, row.id, null, WireJson.encodeToString(row)) } }
    }
    /** Replaces only the rows the fetched window provably covers for its member; rows of other members and outside the
     * window stay until their own refetch, eviction or cleanup. */
    private fun replaceAgendaRows(window: AgendaWindow, members: List<MemberRead>, ownMember: String?) {
        val member = window.memberId ?: ownMember
        for (row in q.allAgendas().executeAsList()) {
            if (member != null && row.id.substringBefore(':') != member) continue
            val agenda = WireJson.decodeFromString<ItemRead>(row.payload).agenda ?: continue
            if (provablyInAgendaWindow(agenda, window.from, window.to, zoneOf(members, agenda.memberId))) q.deleteAgenda(row.id)
        }
        window.rows.forEach { row ->
            row.agenda?.let { q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, members), WireJson.encodeToString(row)) }
        }
    }
    suspend fun cleanup(at: Instant = now()) = transaction { cleanRows(at) }
    suspend fun clear() = transaction(fenced = false) {
        occurrenceObservations.clear()
        clearEntities(); q.clearPlanning(); q.clearAgendas(); q.clearBoards(); q.clearSync(); q.clearStaging()
        clearReceipts()
    }
    private fun clearReceipts() {
        availabilityReceipts.clear(); suggestionReceipts.clear(); itemReceipts.clear(); occurrenceReceipts.clear(); itemDeletions.clear(); visibilityDeletions.clear(); memberReceipts.clear(); settingsReceipt = null
        entryReceipts.clear(); listReceipts.clear(); commentReceipts.clear(); attachmentReceipts.clear(); collaborationDeletions.clear()
    }
    /** Keeps the highest version per id (an idempotent replay may return an older stored response) with the newest
     * receipt number, so it still counts for a snapshot requested between the two. */
    private fun <T> MutableMap<String, Pair<Long, T>>.receive(id: String, value: T, version: (T) -> Int) {
        val previous = this[id]?.second
        this[id] = ++receiptSeq to (if (previous != null && version(previous) > version(value)) previous else value)
    }
    suspend fun replaceNotifications(rows: List<Notification>) = transaction {
        q.clearNotifications(); rows.forEach { q.putNotification(it.id, it.itemId, null, WireJson.encodeToString(it)) }
    }
    suspend fun putNotificationPrefs(row: PrefsRead) = transaction { q.putNotificationPrefs(row.memberId, null, null, WireJson.encodeToString(row)) }
    suspend fun acknowledgeCursor(cursor: Long) = transaction { require(cursor >= 0); q.advanceCursor(cursor) }
    /** A 204 occurrence mutation can regenerate away this identity; remove all of its projections. */
    suspend fun evictOccurrence(id: String) = transaction {
        q.deleteOccurrence(id)
        occurrenceReceipts.remove(id)
        q.allBoards().executeAsList().forEach { row ->
            if (WireJson.decodeFromString<ItemRead>(row.payload).occurrence?.id == id) q.evictBoard(row.item_id)
        }
        q.allAgendas().executeAsList().forEach { row ->
            if (WireJson.decodeFromString<ItemRead>(row.payload).agenda?.occurrenceId == id) q.deleteAgenda(row.id)
        }
        q.allReminders().executeAsList().forEach { row ->
            if (WireJson.decodeFromString<ScheduledReminderRead>(row.payload).occurrenceId == id) q.deleteReminder(row.id)
        }
    }
    suspend fun putAvailability(row: AvailabilityResponse) = transaction {
        availabilityReceipts.receive(row.memberId, row) { it.version }
        val old = mutable.value.availability.find { it.memberId == row.memberId }
        if (old == null || row.version >= old.version) q.putPlanning("availability", row.memberId, null, WireJson.encodeToString(row))
    }
    suspend fun putSuggestion(row: SuggestionResponse) = transaction {
        suggestionReceipts.receive(row.id, row) { it.version }
        val old = mutable.value.suggestions.find { it.id == row.id }
        if (old == null || row.version >= old.version) q.putPlanning("suggestion", row.id, row.itemId, WireJson.encodeToString(row))
    }
    suspend fun replaceSuggestions(member: String?, rows: List<SuggestionResponse>) = transaction {
        mutable.value.suggestions.filter { member == null || it.memberId == member }.forEach { q.removePlanning("suggestion", it.id) }
        rows.forEach { q.putPlanning("suggestion", it.id, it.itemId, WireJson.encodeToString(it)) }
    }
    suspend fun evictItem(id: String) = withContext(NonCancellable) {
        var attachments: List<Attachment> = emptyList()
        transaction {
            attachments = mutable.value.attachments.filter { it.itemId == id }
            evictItemRows(id)
            visibilityDeletions[id] = ++receiptSeq
        }
        // Repository-confirmed visibility loss awaits file removal, not just an optional lifecycle observer.
        removeEvictedItemFiles(attachments)
    }
    private fun evictItemRows(id: String) {
        q.evictNotifications(id)
        q.evictPlanning(id)
        q.evictEntriesByItem(id)
        q.evictList(id); q.evictComment(id); q.evictAttachment(id)
        q.evictItem(id); q.evictOccurrence(id); q.evictAgenda(id); q.evictBoard(id); q.evictReminder(id); q.evictWrite(id)
    }
    suspend fun putList(row: ListResponse) = transaction { putListRow(row); listReceipts.receive("${row.id}:${row.occurrenceId.orEmpty()}", row) { it.version }; collaborationDeletions.remove("list" to row.id) }
    suspend fun replaceLists(itemId: String?, rows: List<ListResponse>) = transaction {
        q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }
            .filter { (itemId == null || it.itemId == itemId) && rows.none { row -> row.id == it.id } }.distinctBy { it.id }.forEach {
                q.deleteList(it.id); q.evictEntry(it.id); collaborationDeletions["list" to it.id] = ++receiptSeq
            }
        rows.forEach { putListRow(it); listReceipts.receive("${it.id}:${it.occurrenceId.orEmpty()}", it) { row -> row.version }; collaborationDeletions.remove("list" to it.id) }
    }
    suspend fun putEntry(row: EntryResponse) = transaction { putEntryRow(row); entryReceipts.receive("${row.id}:${row.occurrenceId.orEmpty()}", row) { it.version } }
    private fun putEntryRow(row: EntryResponse) {
        val list = q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.find { it.id == row.listId && it.occurrenceId == row.occurrenceId } ?: return
        val previous = (list.propertyEntries + list.gotIt).find { it.id == row.id }
        if (previous != null && previous.version > row.version) return
        val rows = ((list.propertyEntries + list.gotIt).filter { it.id != row.id } + if (row.archivedAt == null) listOf(row) else emptyList()).sortedWith(compareBy({ it.position }, { it.id }))
        putListRow(list.copy(propertyEntries = rows.filter { list.mode == ListMode.per_occurrence || it.checkedAt == null },
            gotIt = if (list.mode == ListMode.persistent) rows.filter { it.checkedAt != null } else emptyList()))
    }
    private fun putListRow(row: ListResponse) {
        q.putList(row.id, row.occurrenceId.orEmpty(), row.itemId, null, WireJson.encodeToString(row))
        q.evictEntryScope(row.id, row.occurrenceId.orEmpty())
        (row.propertyEntries + row.gotIt).forEach { q.putEntry(it.id, row.occurrenceId.orEmpty(), row.id, null, WireJson.encodeToString(it)) }
        trimListScopes()
    }
    private fun scopedListRows() = q.allLists().executeAsList().filter { it.occurrence_id.isNotEmpty() }
    /** A household-wide hard cap bounds tickles by requested ids + 32, independent of browsing history.
     * ORDER BY rowid / INSERT OR REPLACE keeps the most recently fetched scopes. Templates do not count. */
    private fun trimListScopes() {
        val scopes = scopedListRows()
        scopes.take((scopes.size - 32).coerceAtLeast(0)).forEach { evictListScopeRows(it.id, it.occurrence_id) }
    }
    private fun evictListScopeRows(id: String, occurrenceId: String) {
        q.deleteListScope(id, occurrenceId); q.evictEntryScope(id, occurrenceId)
        listReceipts.remove("$id:$occurrenceId")
        val keys = entryReceipts.filterValues { it.second.listId == id && it.second.occurrenceId == occurrenceId }.keys.toList()
        keys.forEach { entryReceipts.remove(it) }
    }
    suspend fun evictListScope(id: String, occurrenceId: String) = transaction { evictListScopeRows(id, occurrenceId) }
    suspend fun evictList(id: String) = transaction { q.deleteList(id); q.evictEntry(id); collaborationDeletions["list" to id] = ++receiptSeq }
    suspend fun evictComment(id: String) = transaction { q.deleteComment(id); evictCommentAttachments(id); collaborationDeletions["comment" to id] = ++receiptSeq }
    private fun evictCommentAttachments(id: String) {
        q.allAttachments().executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) }.filter { it.commentId == id }.forEach { q.deleteAttachment(it.id) }
    }
    suspend fun putComments(rows: List<Comment>) = transaction {
        rows.forEach { q.putComment(it.id, it.itemId, null, WireJson.encodeToString(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    suspend fun replaceComments(itemId: String, rows: List<Comment>) = transaction {
        q.allComments().executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) }.filter { it.itemId == itemId && rows.none { row -> row.id == it.id } }.forEach {
            collaborationDeletions["comment" to it.id] = ++receiptSeq; evictCommentAttachments(it.id)
        }
        q.evictComment(itemId); rows.forEach { q.putComment(it.id, it.itemId, null, WireJson.encodeToString(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    suspend fun putAttachment(row: Attachment) = transaction { q.putAttachment(row.id, row.itemId, null, WireJson.encodeToString(row)); attachmentReceipts.receive(row.id, row) { it.version }; collaborationDeletions.remove("attachment" to row.id) }
    suspend fun evictAttachment(id: String) = withContext(NonCancellable) {
        var rows: List<Attachment> = emptyList()
        transaction {
            rows = mutable.value.attachments.filter { it.id == id }
            q.deleteAttachment(id); collaborationDeletions["attachment" to id] = ++receiptSeq
        }
        removeEvictedItemFiles(rows)
    }
    /** Serialize private viewer creation with metadata eviction, even without a trust controller in tests. */
    internal suspend fun withLiveAttachment(row: Attachment, action: suspend () -> Unit) = withContext(Dispatchers.Default) {
        mutex.withLock {
            val fence = sessionCommitMutex
            fence?.lock()
            try {
                currentCoroutineContext().ensureActive()
                val lease = currentCoroutineContext()[Lease]
                if (lease != null && lease.epoch != sessionEpoch()) throw CancellationException("Household session changed")
                require(mutable.value.attachments.any { it.id == row.id && it.version == row.version && it.deletedAt == null }) { "Attachment is no longer visible" }
                action()
            } finally { fence?.unlock() }
        }
    }
    suspend fun replaceMembers(rows: List<MemberRead>, receivedAfter: Long? = null) = transaction {
        val incoming = rows.associateBy { it.id }
        q.clearMembers(); rows.forEach { q.putMember(it.id, null, null, WireJson.encodeToString(it)) }
        receivedAfter?.let { mark ->
            for ((seq, row) in memberReceipts.values) {
                val installed = incoming[row.id] ?: continue
                if (seq > mark && row.version > installed.version) q.putMember(row.id, null, null, WireJson.encodeToString(row))
            }
        }
        // n-2: receipts stay for an in-flight snapshot that predates them (its install re-applies or clears them);
        // a later collection's own mark already excludes earlier receipts.
    }
    suspend fun putMember(row: MemberRead) = transaction {
        val installed = q.allMembers().executeAsList().find { it.id == row.id }?.let { WireJson.decodeFromString<MemberRead>(it.payload) }
        if (installed == null || installed.version <= row.version) q.putMember(row.id, null, null, WireJson.encodeToString(row))
        memberReceipts.receive(row.id, row) { it.version }
    }
    suspend fun putSettings(row: SettingsRead) = transaction {
        val installed = q.allSettingss().executeAsList().firstOrNull()?.let { WireJson.decodeFromString<SettingsRead>(it.payload) }
        if (installed == null || installed.memberId != row.memberId || installed.version <= row.version) {
            q.clearSettingss(); q.putSettings(row.memberId, null, null, WireJson.encodeToString(row))
        }
        val previous = settingsReceipt?.second
        settingsReceipt = ++receiptSeq to
            (if (previous != null && previous.memberId == row.memberId && previous.version > row.version) previous else row)
    }
    suspend fun replaceBoard(rows: List<ItemRead>) = transaction { replaceBoardRows(rows) }
    /** Agenda refetch: replaces in place only what [window] authoritatively covered. */
    suspend fun replaceAgenda(window: AgendaWindow) = transaction {
        mutable.value.let { replaceAgendaRows(window, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** Replaces every cached agenda row (test seeding; production refetches use [AgendaWindow]). */
    internal suspend fun replaceAgenda(rows: List<ItemRead>) = transaction {
        q.clearAgendas()
        rows.forEach { row -> row.agenda?.let { q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, mutable.value.members), WireJson.encodeToString(row)) } }
        cleanRows(now())
    }
    /** Write follow-up (R59): board and agenda switch together, in place, never cleared ahead of a fetch. */
    suspend fun replaceProjections(projections: Projections) = transaction {
        replaceBoardRows(projections.board)
        mutable.value.let { replaceAgendaRows(projections.agenda, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** A windowed `/v1/items/{id}/occurrences` answer never contains undated occurrences, nor anything outside
     * [from, to): only returned rows and cached rows the window provably covered (now gone) are replaced (I-2). The
     * window is required: there is no "replace everything" default that could drop undated rows (N-3). */
    suspend fun putOccurrences(itemId: String, rows: List<OccurrenceResponse>, from: Instant, to: Instant,
                               includeUndated: Boolean = false, receivedAfter: Long? = null) = transaction {
        val returned = rows.mapTo(HashSet()) { it.id }
        // m-7: whether dated-window or undated coverage, a missing row is deleted only if nothing newer than the request
        // (an own-write response or a snapshot install) observed it.
        mutable.value.occurrences.filter { it.itemId == itemId && it.id !in returned &&
            (provablyInWindow(it, from, to) || (includeUndated && it.startsAt == null && it.dueAt == null && it.localStart == null && it.localDue == null && it.dueDate == null)) &&
            (receivedAfter?.let { mark -> (occurrenceObservations[it.id] ?: 0L) <= mark } ?: (it.id !in occurrenceReceipts)) }
            .forEach { q.deleteOccurrence(it.id) }
        val items = mutable.value.items.associateBy { it.id }
        val occurrences = mutable.value.occurrences.associateBy { it.id }
        rows.forEach { putOccurrenceRow(it, items, occurrences) }
        cleanRows(now())
    }
    suspend fun putItem(response: ItemResponse) = putItems(listOf(response))
    /** One transaction for a whole page of item reads/writes. */
    suspend fun putItems(responses: List<ItemResponse>) = transaction {
        val existing = mutable.value.items.associateBy { it.id }
        responses.forEach { putItemRow(it, existing[it.id]); itemReceipts.receive(it.id, it) { row -> row.version } }
    }
    private fun putItemRow(response: ItemResponse, existing: ItemRead?) {
        val installed = q.getItem(response.id).executeAsOneOrNull()?.let { WireJson.decodeFromString<ItemRead>(it.payload) } ?: existing
        if (installed != null && installed.version > response.version) return
        if (response.deletedAt != null) {
            evictItemRows(response.id)
            // O-g: an in-flight older snapshot must not bring the deleted item back (see installStaged).
            itemDeletions.receive(response.id, response.version) { it }
            return
        }
        val raw = responseJson.encodeToJsonElement(response).jsonObject
        val merged = (installed?.let { WireJson.encodeToJsonElement(it).jsonObject }.orEmpty() + raw).toMutableMap()
        // Item responses do not carry creation metadata/reminder rows. Retain snapshot values, otherwise
        // use first-observed time until the next snapshot; never manufacture reminder identities.
        merged.putIfAbsent("created_at", JsonPrimitive(now().toString()))
        merged["updated_at"] = JsonPrimitive(now().toString())
        val retained = installed?.reminders.orEmpty().filter { it.offsetMinutes in response.reminderOffsets }
        merged["reminders"] = WireJson.encodeToJsonElement(retained)
        merged.remove("occurrence"); merged.remove("agenda")
        val row = WireJson.decodeFromJsonElement<ItemRead>(JsonObject(merged))
        q.putItem(row.id, row.id, null, WireJson.encodeToString(row))
        visibilityDeletions.remove(row.id)
        // itemWrites: the latest item-shaped response (write or GET) with its reminder offsets, until the next snapshot.
        q.putWrite(response.id, response.id, null, WireJson.encodeToString(response))
    }
    /** A write's authoritative response for one occurrence (also kept as a receipt, see [installStaged]). */
    suspend fun putOccurrence(response: OccurrenceResponse) = transaction {
        putOccurrenceRow(response, mutable.value.items.associateBy { it.id }, mutable.value.occurrences.associateBy { it.id })
        occurrenceReceipts.receive(response.id, response) { it.version }
    }
    private fun putOccurrenceRow(response: OccurrenceResponse, items: Map<String, ItemRead>, occurrences: Map<String, OccurrenceRead>,
                                 zone: TimeZone = ownZone()) {
        val existing = q.getOccurrence(response.id).executeAsOneOrNull()?.let { WireJson.decodeFromString<OccurrenceRead>(it.payload) } ?: occurrences[response.id]
        if (existing != null && existing.version > response.version) return
        val raw = responseJson.encodeToJsonElement(response).jsonObject
        val merged = (existing?.let { WireJson.encodeToJsonElement(it).jsonObject }.orEmpty() + raw).toMutableMap()
        if ("household_id" !in merged) {
            // Occurrence responses omit household_id: every cached row belongs to the one household.
            val household = items[response.itemId]?.householdId ?: items.values.firstOrNull()?.householdId
                ?: mutable.value.members.firstOrNull()?.householdId ?: return // unknown: the follow-up refetch supplies it
            merged["household_id"] = JsonPrimitive(household)
        }
        merged.putIfAbsent("board_moved", JsonPrimitive(false))
        merged.putIfAbsent("created_at", JsonPrimitive(now().toString()))
        merged["updated_at"] = JsonPrimitive(now().toString())
        val row = WireJson.decodeFromJsonElement<OccurrenceRead>(JsonObject(merged))
        q.putOccurrence(row.id, row.itemId, expiry(row, zone, items[row.itemId]), WireJson.encodeToString(row))
        occurrenceObservations[row.id] = ++receiptSeq
        // Snapshot receipt reapplication may pass an older item map; only update the installed parent's flag.
        q.getItem(row.itemId).executeAsOneOrNull()?.let { stored ->
            val item = WireJson.decodeFromString<ItemRead>(stored.payload)
            q.putItem(item.id, item.id, null, WireJson.encodeToString(item.copy(pendingDerivation = response.pendingDerivation)))
        }
    }
    fun close() = runBlocking(Dispatchers.Default) { mutex.withLock {
        if (!closed) { closed = true; driver?.close() }
    } }
}

private fun local(value: String): LocalDateTime? = runCatching { LocalDateTime.parse(value) }.getOrNull()

/** True only when the server's windowed occurrence query (server items/read.rs) certainly includes [row]: its start
 * (starts_at, due_at, local start/due, due date — the server's order) lies in [from, to) even under any zone offset
 * for wall-clock/date-only values. Undated rows are never covered (the windowed endpoint never returns them). */
internal fun provablyInWindow(row: OccurrenceRead, from: Instant, to: Instant): Boolean {
    val wallClock = row.localStart ?: row.localDue
    val (start, margin) = when {
        row.startsAt != null -> row.startsAt to Duration.ZERO
        row.dueAt != null -> row.dueAt to Duration.ZERO
        wallClock != null -> (local(wallClock)?.toInstant(TimeZone.UTC) ?: return false) to 1.days
        row.dueDate != null -> row.dueDate.atStartOfDayIn(TimeZone.UTC) to 1.days
        else -> return false
    }
    return start - margin >= from && start + margin < to
}

/** Mirror of the server's agenda window filter (server routes/reads.rs), including member-local date overlap. */
internal fun provablyInAgendaWindow(agenda: AgendaRead, from: Instant, to: Instant, zone: TimeZone = TimeZone.UTC): Boolean {
    val starts = agenda.startsAt; val ends = agenda.endsAt
    if (starts != null && ends != null && starts < to && ends > from) return true
    agenda.dueAt?.let { if (it >= from && it < to) return true }
    agenda.dueDate?.let { date ->
        val day = date.atStartOfDayIn(zone)
        val end = (date + DatePeriod(days = 1)).atStartOfDayIn(zone)
        if ((starts ?: agenda.dueAt ?: day) < to && end > from) return true
    }
    return false
}
