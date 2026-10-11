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
data class CommentHistory(val nextCursor: String? = null, val started: Boolean = false,
                          val complete: Boolean = false, val olderPages: Int = 0)
data class CacheView(
    val members: List<MemberRead> = emptyList(), val items: List<ItemRead> = emptyList(),
    val occurrences: List<OccurrenceRead> = emptyList(), val agenda: List<ItemRead> = emptyList(),
    val board: List<ItemRead> = emptyList(), val settings: SettingsRead? = null,
    val reminders: List<ScheduledReminderRead> = emptyList(), val itemWrites: Map<String, ItemResponse> = emptyMap(), val sync: SyncState? = null,
    val notifications: List<Notification> = emptyList(), val notificationPrefs: PrefsRead? = null,
    val availability: List<AvailabilityResponse> = emptyList(), val suggestions: List<SuggestionResponse> = emptyList(),
    val lists: List<ListResponse> = emptyList(), val comments: List<Comment> = emptyList(), val attachments: List<Attachment> = emptyList(),
    val evictedListIds: Set<String> = emptySet(), val evictedItemIds: Set<String> = emptySet(),
    val visibilityRevision: Long = 0, val commentHistory: Map<String, CommentHistory> = emptyMap(),
) {
    // Process-local draft fences are not part of payload equality (nor persisted cache data).
    internal var resourceVisibility: Map<CacheResource, Any> = emptyMap()
    internal var identityVisibility: Any? = null
}
/** One fully paginated `/v1/agenda` answer: the server authoritatively covered [from, to) for [memberId]
 * (null = the caller's own member). */
data class AgendaWindow(val from: Instant, val to: Instant, val memberId: String?, val rows: List<ItemRead>)
/** Board (one scope) and agenda (one window), both fetched completely before either replaces cached rows (R59). */
data class Projections(val board: List<ItemRead>, val agenda: AgendaWindow)

/** Item responses keep explicit nulls/defaults when merged over a cached row. */
private val responseJson = Json(WireJson) { encodeDefaults = true }

class StaleVisibilityReply : Exception("Resource visibility changed during request")

/** Platform code owns exactly one disposable DB path, never trust, settings or alarm inventories. */
interface CacheRecovery {
    fun isCorruption(error: Exception): Boolean
    fun discard()
}
const val CACHE_STORAGE_NOTICE = "Household cache unavailable. Check storage; Porchlight will retry automatically."

/** One driver per application. Transactions publish one immutable view, never partially loaded projections. */
class ReadCache(private val open: () -> SqlDriver, private val now: () -> Instant, private val recovery: CacheRecovery?,
                internal val attachmentStore: AttachmentStore = MemoryAttachmentStore()) {
    constructor(open: () -> SqlDriver, now: () -> Instant) : this(open, now, null)
    /** Already opened drivers are useful for memory-only tests; production uses a reopenable factory. */
    constructor(driver: SqlDriver, now: () -> Instant) : this({ driver }, now, null)
    private var driver: SqlDriver? = null
    private var database: PorchlightDb? = null
    private var recoveryAttempted = false
    private var storageGeneration = 0L
    private val mutableStorageRevision = MutableStateFlow(0L)
    /** Reconnect even a healthy stream when its disposable cursor/cache has been lost. */
    val storageRevision: StateFlow<Long> = mutableStorageRevision.asStateFlow()
    // Opening and recovery run under the cache mutex, off the composition thread.
    private val db get() = database ?: PorchlightDb(open().also { driver = it }).also {
        database = it
        receiptSeq = maxOf(receiptSeq, it.cacheQueries.visibilityGeneration().executeAsOne())
        visibilityRevision = receiptSeq
    }
    private fun closeDriver() { driver?.close(); driver = null; database = null }
    private val mutableStorageNotice = MutableStateFlow<String?>(null)
    val storageNotice: StateFlow<String?> = mutableStorageNotice.asStateFlow()
    private val q get() = db.cacheQueries
    private val mutex = Mutex()
    // Installed by the application binding before any cache readers/collectors start.
    internal var ownerBindingRequired = false
    private var expectedOwner: CacheOwner? = null
    private var ownerReady = false
    // A completed identity clear can unblock live inventory cleanup without admitting household reads.
    private var ownerlessInitialized = false
    private var identityClearPending = false
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
    private val attachmentCleanup = AttachmentCleanup(attachmentStore)
    private val itemDeletions = mutableMapOf<String, Pair<Long, Int>>() // item id -> (receipt, deleted version)
    private val memberReceipts = mutableMapOf<String, Pair<Long, MemberRead>>()
    private val entryReceipts = mutableMapOf<String, Pair<Long, EntryResponse>>()
    private val listReceipts = mutableMapOf<String, Pair<Long, ListResponse>>()
    // Parent ACL changes do not bump list versions. Unlike ordinary receipts these
    // fences survive snapshot installation, until a read STARTED after the fence
    // explicitly restores access (or the household session is cleared).
    private val commentReceipts = mutableMapOf<String, Pair<Long, Comment>>()
    private val attachmentReceipts = mutableMapOf<String, Pair<Long, Attachment>>()
    private val collaborationDeletions = mutableMapOf<Pair<String, String>, Long>()
    private var settingsReceipt: Pair<Long, SettingsRead>? = null
    var sessionEpoch: () -> Long = { 0L }
    internal var sessionCommitMutex: Mutex? = null
    private var committingVisibility: Long? = null
    private var visibilityRevision = 0L
    private var visibilityFloor = 0L
    // Tokens cover live resources only, not unbounded eviction history. Evict/re-read creates a new token.
    private var resourceVisibility = emptyMap<CacheResource, Any>()
    private var identityVisibility: Any = Any()
    // Bound durable tombstones and restored history. Requests preceding a pruned fence are
    // conservatively rejected by the process-local floor, even after a newer reader restores access.
    private fun trimVisibility() {
        q.visibilityCutoff().executeAsOneOrNull()?.let { cutoff ->
            visibilityFloor = maxOf(visibilityFloor, cutoff)
            q.pruneVisibility(cutoff)
        }
    }
    private fun evicted(kind: String, id: String) {
        q.putVisibility(kind, id, ++receiptSeq)
        visibilityRevision = receiptSeq
        resourceVisibility = resourceVisibility - CacheResource(kind, id)
    }
    private fun requireVisible(kind: String, id: String, itemId: String? = null, receivedAt: Long? = committingVisibility) {
        val mark = receivedAt ?: return
        if (mark < visibilityFloor || (q.getVisibility(kind, id).executeAsOneOrNull() ?: 0) > mark ||
            (itemId != null && (q.getVisibility("Item", itemId).executeAsOneOrNull() ?: 0) > mark))
            throw StaleVisibilityReply()
    }
    sealed interface Fence
    private class Lease(val owner: ReadCache, val epoch: Long, val visibility: Long, val storage: Long) : AbstractCoroutineContextElement(Key), Fence {
        companion object Key : CoroutineContext.Key<Lease>
    }
    /** The only request-start API. Nested reads inherit the original fence, never a receipt-time fence. */
    suspend fun <T> inSession(block: suspend (Fence) -> T): T {
        currentCoroutineContext()[Lease]?.takeIf { it.owner === this }?.let { return block(it) }
        val epoch = sessionEpoch()
        val lease = withContext(Dispatchers.Default) {
            mutex.withLock {
                if (closed) throw CancellationException("Read cache closed")
                if (ownerBindingRequired && !ownerReady) throw CacheOwnerUnavailable()
                db // open/seed the monotonic counter once, never scan history per request
                Lease(this@ReadCache, epoch, receiptSeq, storageGeneration)
            }
        }
        return withContext(lease) { block(lease) }
    }
    // Always reload/reconcile: the disposable database can change outside this driver.
    suspend fun initialize(owner: CacheOwner? = null) = transaction(fenced = owner == null, recover = true, binding = owner) { trimListScopes() }
    /** Application-lifetime collectors must survive temporary storage failures. Workers use initialize + retry. */
    suspend fun awaitInitialized() {
        var wait = 1_000L
        while (true) {
            try {
                initialize()
                mutableStorageNotice.value = null
                return
            } catch (e: CancellationException) { throw e }
            catch (_: Exception) { mutableStorageNotice.value = CACHE_STORAGE_NOTICE }
            delay(wait)
            wait = (wait * 2).coerceAtMost(30_000)
        }
    }
    fun itemWrite(id: String): ItemResponse? = view.value.itemWrites[id]
    val view: StateFlow<CacheView> = mutable.asStateFlow()
    val sync: StateFlow<SyncState?> = mutableSync.asStateFlow()
    private fun publish(next: CacheView, resetVisibility: Boolean = false) {
        if (resetVisibility) { visibilityRevision = ++receiptSeq; resourceVisibility = emptyMap(); identityVisibility = Any() }
        val resources = buildSet {
            (next.items + next.board + next.agenda).forEach { add(CacheResource("Item", it.id)) }
            next.itemWrites.keys.forEach { add(CacheResource("Item", it)) }
            next.occurrences.forEach { add(CacheResource("Occurrence", it.id)) }
            next.comments.filter { it.deletedAt == null }.forEach { add(CacheResource("Comment", it.id)) }
            next.lists.filter { it.redacted != true }.forEach { add(CacheResource("List", it.id)) }
        }
        resourceVisibility = resources.associateWith { resourceVisibility[it] ?: Any() }
        mutable.value = next.copy(visibilityRevision = visibilityRevision).also {
            it.resourceVisibility = resourceVisibility; it.identityVisibility = identityVisibility
        }
        mutableSync.value = next.sync
    }
    private class InvalidCachePayload(cause: Exception) : Exception("Disposable cache payload is invalid", cause)
    private fun load(): CacheView = try {
        val planning = q.allPlanning().executeAsList()
        CacheView(
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
        evictedListIds = q.evictedVisibility("List").executeAsList().toSet(),
        evictedItemIds = q.evictedVisibility("Item").executeAsList().toSet(),
        commentHistory = q.allCommentHistories().executeAsList().associate { it.item_id to
            CommentHistory(it.next_cursor, it.started != 0L, it.complete != 0L, it.older_pages.toInt()) },
        )
    } catch (e: IllegalArgumentException) { throw InvalidCachePayload(e) }
    /** [fenced] = false: the caller's inherited session lease does not apply (identity clears must always land). */
    private suspend fun transaction(fence: Fence, block: () -> Unit) {
        val lease = fence as Lease
        check(lease.owner === this && currentCoroutineContext()[Lease] === lease) { "Use the request-start fence inside inSession" }
        transaction(block = block)
    }
    // Only lifecycle/removal operations may use the unfenced transaction overload.
    private suspend fun transaction(fenced: Boolean = true, recover: Boolean = false, binding: CacheOwner? = null, block: () -> Unit) = withContext(Dispatchers.Default) { mutex.withLock {
        val identityClear = !fenced && binding == null
        if (binding != null) {
            ownerBindingRequired = true
            if (expectedOwner != binding) ownerReady = false
            ownerlessInitialized = false
            expectedOwner = binding
        }
        val context = currentCoroutineContext()
        val lease = if (fenced) context[Lease] else null
        fun checkLease() {
            if (closed) throw CancellationException("Read cache closed")
            context.ensureActive()
            if (lease != null && (lease.epoch != sessionEpoch() || lease.storage != storageGeneration))
                throw CancellationException("Household session or cache changed")
        }
        val fence = sessionCommitMutex
        fence?.lock()
        try {
            checkLease()
            if (identityClear) { expectedOwner = null; identityClearPending = true; hideOwner() }
            committingVisibility = lease?.visibility
            var cleanupRequired = recover || identityClear
            while (true) {
                try {
                    val database = db
                    if (ownerBindingRequired && !identityClear) reconcileOwner()
                    checkLease()
                    val priorVisibility = visibilityRevision
                    val priorFloor = visibilityFloor
                    val priorResources = resourceVisibility
                    var reset = false
                    val next = try { database.transactionWithResult<CacheView> {
                        // Owner and cursor checks share the operation's SQLite snapshot with ALL payload reads.
                        // A separate probe could consume an external reset without notifying the live stream.
                        if (ownerBindingRequired && !identityClear && expectedOwner != null) {
                            val owner = q.getCacheOwner().executeAsOneOrNull()
                            if (owner?.ca_fingerprint != expectedOwner?.caFingerprint || owner?.device_id != expectedOwner?.deviceId || owner?.member_id != expectedOwner?.memberId) {
                                ownerReady = false
                                throw CacheOwnerUnavailable()
                            }
                        }
                        reset = mutable.value.sync != null && q.getSync().executeAsOneOrNull() == null
                        if (!reset) {
                            if (ownerBindingRequired && expectedOwner == null) {
                                // Initialized ownerless is empty, never permission to load anonymous disk rows.
                                clearRows(); q.clearCacheOwner()
                            } else block()
                            checkLease(); trimVisibility()
                        }
                        load()
                    } } catch (e: Exception) {
                        visibilityRevision = priorVisibility; visibilityFloor = priorFloor; resourceVisibility = priorResources
                        throw e
                    }
                    if (reset) {
                        discardPublished(next)
                        withContext(NonCancellable) { attachmentCleanup.drain() }
                        checkLease() // The pre-reset operation cannot refill the cache with an old reply.
                        continue
                    }
                    cleanupRequired = attachmentCleanup.transition(mutable.value, next, sessionEpoch(), clearIdentity = identityClear) || cleanupRequired
                    publish(next, resetVisibility = identityClear)
                    break
                } catch (e: CancellationException) { throw e }
                catch (e: Exception) {
                    if (ownerBindingRequired && !ownerReady) { hideOwner(); mutableStorageNotice.value = CACHE_STORAGE_NOTICE }
                    if (!recover) throw e
                    // Always close before a retry/delete. I/O, busy, full and permission failures never justify a wipe.
                    closeDriver()
                    if (recovery == null || recoveryAttempted ||
                        !(e is InvalidCachePayload || recovery.isCorruption(e))) throw e
                    recoveryAttempted = true
                    recovery.discard()
                    discardPublished()
                    cleanupRequired = true
                }
            }
            // Commit already landed: cancellation must not skip private artifact removal.
            // An earlier failed purge must not make unrelated projection reads fail.
            if (cleanupRequired) withContext(NonCancellable) { attachmentCleanup.drain() }
            if (identityClear) {
                ownerlessInitialized = true
                identityClearPending = false
            }
            mutableStorageNotice.value = null
        } catch (e: Exception) {
            if (ownerBindingRequired && !ownerReady) { hideOwner(); mutableStorageNotice.value = CACHE_STORAGE_NOTICE }
            throw e
        } finally { committingVisibility = null; fence?.unlock() }
    } }
    private fun discardPublished(next: CacheView = CacheView()) {
        storageGeneration++
        clearReceipts(); occurrenceObservations.clear()
        attachmentCleanup.transition(mutable.value, next, sessionEpoch(), clearIdentity = true)
        publish(next, resetVisibility = true)
        mutableStorageRevision.value = storageGeneration
    }
    private fun hideOwner() {
        ownerReady = false
        ownerlessInitialized = false
        discardPublished()
    }
    /** The missing owner row is a durable file-cleanup obligation: write the replacement only after purge succeeds. */
    private suspend fun reconcileOwner() {
        val owner = expectedOwner ?: run {
            if (ownerlessInitialized) return
            throw CacheOwnerUnavailable()
        }
        val stored = q.getCacheOwner().executeAsOneOrNull()
        val sameDevice = stored?.ca_fingerprint == owner.caFingerprint && stored.device_id == owner.deviceId
        val sameOwner = sameDevice && (stored?.member_id == owner.memberId || stored?.member_id == null)
        val clearRequired = !sameOwner || identityClearPending
        if (clearRequired) {
            hideOwner()
            db.transaction { clearRows(); q.clearCacheOwner() }
            withContext(NonCancellable) { attachmentCleanup.drain() }
        }
        if (clearRequired || stored?.member_id != owner.memberId)
            db.transaction { q.putCacheOwner(owner.caFingerprint, owner.memberId, owner.deviceId) }
        identityClearPending = false
        ownerReady = true
    }
    private inline fun <reified T> stage(kind: String, id: String, itemId: String?, expiry: Long?, value: T) {
        if (value !is ListResponse || value.redacted != true) {
            requireVisible(kind, id, if (kind == "Entry") null else itemId)
            if (kind == "Entry" && itemId != null) requireVisible("List", itemId)
            q.restoreVisibility(kind, id)
        }
        q.stage(kind, id, itemId, expiry, WireJson.encodeToString(value), (value as? ScheduledReminderRead)?.occurrenceId)
    }

    /** Authoritative switch (R59): entities, cursor and — when given — the board/agenda [projections] fetched for this
     * snapshot commit in ONE transaction. Projection rows of items that are no longer visible disappear (snapshot
     * difference); fetched projections replace theirs in place. Throwing anywhere rolls back every table and cursor. */
    suspend fun install(fence: Fence, snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null) =
        installStaged(fence, snapshot, at, projections)
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
    internal suspend fun installStaged(fence: Fence, snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null,
                                       receivedAfter: Long? = null, requestCursor: Long? = null, authoritative: Boolean = false,
                                       scopedChecks: List<ListResponse>? = null, beforeCommit: () -> Unit = {}, afterStage: () -> Unit = {}) = transaction(fence) {
        // Snapshot receipt repair must not reinstall resources evicted after its request, even on resnapshot.
        fun visible(kind: String, id: String, itemId: String? = null): Boolean = try {
            requireVisible(kind, id, itemId); true
        } catch (_: StaleVisibilityReply) { false }
        val snapshot = snapshot.copy(
            items = snapshot.items.filter { visible("Item", it.id) },
            occurrences = snapshot.occurrences.filter { visible("Occurrence", it.id, it.itemId) },
            scheduledReminders = snapshot.scheduledReminders.filter { visible("Occurrence", it.occurrenceId, it.itemId) },
            comments = snapshot.comments.filter { visible("Comment", it.id, it.itemId) },
            notifications = snapshot.notifications.filter { (it.itemId == null || visible("Item", it.itemId)) && (it.occurrenceId == null || visible("Occurrence", it.occurrenceId)) },
            suggestions = snapshot.suggestions.filter { visible("Item", it.itemId) && visible("Occurrence", it.occurrenceId) },
            attachments = snapshot.attachments.filter { visible("Attachment", it.id, it.itemId) && (it.commentId == null || visible("Comment", it.commentId)) },
        )
        val projections = projections?.let { rows -> Projections(
            rows.board.filter { visible("Item", it.id) && (it.occurrence == null || visible("Occurrence", it.occurrence.id)) },
            rows.agenda.copy(rows = rows.agenda.rows.filter { visible("Item", it.id) && (it.agenda == null || visible("Occurrence", it.agenda.occurrenceId)) })) }
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
        (mutable.value.items + mutable.value.board + mutable.value.agenda).map { it.id }.distinct()
            .filter { it !in itemsById }.forEach { evicted("Item", it) }
        mutable.value.lists.map { it.id }.distinct().filter { id -> snapshot.lists.none { it.id == id } }.forEach { evicted("List", it) }
        mutable.value.occurrences.filter { old -> snapshot.occurrences.none { it.id == old.id } }.forEach { evicted("Occurrence", it.id) }
        mutable.value.comments.filter { old -> snapshot.comments.none { it.id == old.id } }.forEach { evicted("Comment", it.id) }
        mutable.value.attachments.filter { old -> snapshot.attachments.none { it.id == old.id } }.forEach { evicted("Attachment", it.id) }
        val scopedLists = q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.filter { it.occurrenceId != null }
        val ownZone = zoneOf(members, settings.memberId)
        // Visibility fences apply even to authoritative/restore snapshots: only a
        // later authorized read, never a greater list version, clears a redaction.
        val lists = snapshot.lists.mapNotNull { try { visibleListEntries(acceptListVisibility(it, receivedAfter)) } catch (_: StaleVisibilityReply) { null } }
        val incomingEntries = lists.flatMap { it.propertyEntries + it.gotIt }.mapTo(mutableSetOf()) { it.id }
        mutable.value.lists.flatMap { it.propertyEntries + it.gotIt }.distinctBy { it.id }
            .filter { it.id !in incomingEntries }.forEach { evictEntryRows(it.id, it.listId) }
        q.clearStaging()
        members.forEach { stage("Member", it.id, null, null, it); afterStage() }
        items.forEach { stage("Item", it.id, it.id, null, it); afterStage() }
        // Parent omissions in this very snapshot may have fenced an inconsistent child above.
        occurrences.filter { visible("Occurrence", it.id, it.itemId) }.forEach { stage("Occurrence", it.id, it.itemId, expiry(it, ownZone, itemsById[it.itemId]), it); afterStage() }
        snapshot.scheduledReminders.forEach { stage("Reminder", it.id, it.itemId, it.fireAt.toEpochMilliseconds(), it); afterStage() }
        lists.forEach { row ->
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
        q.promoteEntities()
        q.clearPlanning()
        snapshot.availability.forEach { q.putPlanning("availability", it.memberId, null, WireJson.encodeToString(it)) }
        snapshot.suggestions.forEach { q.putPlanning("suggestion", it.id, it.itemId, WireJson.encodeToString(it)) }
        q.clearCommentHistories()
        snapshot.items.forEach { item -> putCommentHistory(item.id, CommentHistory(started = true, complete = true,
            olderPages = mutable.value.commentHistory[item.id]?.olderPages ?: 0)) }
        // Snapshot lists are templates, not occurrence check projections. Retain only scopes
        // within this authoritative occurrence window, merging structure/ACLs and independent checks.
        val visibleOccurrences = snapshot.occurrences.mapTo(mutableSetOf()) { it.id }
        val templates = lists.associateBy { it.id }
        (scopedChecks ?: scopedLists).filter { it.occurrenceId in visibleOccurrences && visible("ListScope", "${it.id}:${it.occurrenceId}") }.forEach { scoped ->
            val template = templates[scoped.id]?.takeIf { it.mode == ListMode.per_occurrence && !it.cleanupOnly }
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
        if (!regressed && !authoritative) receivedAfter?.let { reapplyNewerReceipts(snapshot, it, ownZone) }
        // Every later snapshot is requested after these receipts, so none is needed again.
        clearReceipts()
        cleanRows(at)
        q.clearStaging()
        beforeCommit()
    }
    /** Re-applies only over rows the snapshot holds (M-1). Previously existing undated rows are covered;
     * newly created or out-of-window rows can also be absent. Repair refetch, not receipts, restores them. */
    private fun reapplyNewerReceipts(snapshot: SnapshotRead, mark: Long, zone: TimeZone) {
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
            if (redactedList(row.id) != null) continue
            val installed = lists[row.id] ?: continue
            if (seq > mark && (row.occurrenceId == null || row.occurrenceId in occurrences) &&
                (row.version > installed.version || (row.occurrenceId != null && row.version == installed.version)) &&
                (row.itemId == null || row.itemId in items)) putListRow(row)
        }
        for ((seq, row) in entryReceipts.values.toList()) {
            if (redactedList(row.listId) != null) continue
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
        rows.forEach { row -> requireVisible("Item", row.id); q.restoreVisibility("Item", row.id); row.occurrence?.let { requireVisible("Occurrence", it.id); q.putBoard(it.id, row.id, null, WireJson.encodeToString(row)) } }
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
            requireVisible("Item", row.id); q.restoreVisibility("Item", row.id)
            row.agenda?.let { requireVisible("Occurrence", it.occurrenceId); q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, members), WireJson.encodeToString(row), it.occurrenceId) }
        }
    }
    suspend fun cleanup(at: Instant = now()) = transaction { cleanRows(at) }
    suspend fun clear() = transaction(fenced = false) { clearRows(); q.clearCacheOwner() }
    private fun clearRows() {
        occurrenceObservations.clear()
        clearEntities(); q.clearPlanning(); q.clearAgendas(); q.clearBoards(); q.clearSync(); q.clearStaging(); q.clearVisibility(); q.clearCommentHistories()
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
    suspend fun replaceNotifications(fence: Fence, rows: List<Notification>) = transaction(fence) {
        rows.forEach { row ->
            row.itemId?.let { requireVisible("Item", it) }
            row.occurrenceId?.let { requireVisible("Occurrence", it) }
        }
        q.clearNotifications(); rows.forEach { q.putNotification(it.id, it.itemId, null, WireJson.encodeToString(it)) }
    }
    suspend fun putNotificationPrefs(fence: Fence, row: PrefsRead) = transaction(fence) { q.putNotificationPrefs(row.memberId, null, null, WireJson.encodeToString(row)) }
    suspend fun acknowledgeCursor(cursor: Long) = transaction { require(cursor >= 0); q.advanceCursor(cursor) }
    /** A 204 occurrence mutation can regenerate away this identity; remove all of its projections. */
    suspend fun evictOccurrence(id: String) = transaction { evictOccurrenceRows(id) }
    private fun evictOccurrenceRows(id: String) {
        evicted("Occurrence", id)
        q.deleteOccurrence(id)
        occurrenceReceipts.remove(id)
        mutable.value.notifications.filter { it.occurrenceId == id }.forEach { q.deleteNotification(it.id) }
        mutable.value.suggestions.filter { it.occurrenceId == id }.forEach { q.removePlanning("suggestion", it.id) }
        scopedListRows().filter { it.occurrence_id == id }.forEach { evictListScopeRows(it.id, id) }
        q.evictOccurrenceBoard(id); q.evictOccurrenceAgenda(id); q.evictOccurrenceReminders(id)
    }
    suspend fun putAvailability(fence: Fence, row: AvailabilityResponse) = transaction(fence) {
        availabilityReceipts.receive(row.memberId, row) { it.version }
        val old = mutable.value.availability.find { it.memberId == row.memberId }
        if (old == null || row.version >= old.version) q.putPlanning("availability", row.memberId, null, WireJson.encodeToString(row))
    }
    private fun requireSuggestionVisible(row: SuggestionResponse) {
        requireVisible("Item", row.itemId); requireVisible("Occurrence", row.occurrenceId)
    }
    suspend fun putSuggestion(fence: Fence, row: SuggestionResponse) = transaction(fence) {
        requireSuggestionVisible(row)
        suggestionReceipts.receive(row.id, row) { it.version }
        val old = mutable.value.suggestions.find { it.id == row.id }
        if (old == null || row.version >= old.version) q.putPlanning("suggestion", row.id, row.itemId, WireJson.encodeToString(row))
    }
    suspend fun replaceSuggestions(fence: Fence, member: String?, rows: List<SuggestionResponse>) = transaction(fence) {
        rows.forEach { requireSuggestionVisible(it) }
        mutable.value.suggestions.filter { member == null || it.memberId == member }.forEach { q.removePlanning("suggestion", it.id) }
        rows.forEach { q.putPlanning("suggestion", it.id, it.itemId, WireJson.encodeToString(it)) }
    }
    suspend fun evictItem(id: String) = withContext(NonCancellable) {
        transaction { evictItemRows(id); visibilityDeletions[id] = ++receiptSeq }
    }
    private fun evictItemRows(id: String) {
        evicted("Item", id)
        q.evictNotifications(id)
        q.evictPlanning(id)
        q.evictEntriesByItem(id)
        q.evictList(id); q.evictComment(id); q.evictAttachment(id); q.deleteCommentHistory(id)
        q.evictItem(id); q.evictOccurrence(id); q.evictAgenda(id); q.evictBoard(id); q.evictReminder(id); q.evictWrite(id)
    }
    suspend fun putList(fence: Fence, row: ListResponse, requestedAfter: Long? = null): ListResponse {
        var accepted: ListResponse? = null
        transaction(fence) { accepted = receiveList(row, requestedAfter) }
        return accepted ?: throw CancellationException("List visibility changed")
    }
    suspend fun replaceLists(fence: Fence, itemId: String?, rows: List<ListResponse>, requestedAfter: Long? = null): List<ListResponse> {
        var accepted = emptyList<ListResponse>()
        transaction(fence) {
            val mark = requestedAfter ?: (fence as Lease).visibility
            val newerLists = listReceipts.values.filter { it.first > mark }.map { it.second }
            val newerEntries = entryReceipts.values.filter { it.first > mark }.map { it.second }
            val retained = newerLists.mapTo(mutableSetOf()) { it.id } + newerEntries.map { it.listId }
            q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }
                .filter { (itemId == null || it.itemId == itemId) && it.id !in retained && rows.none { row -> row.id == it.id } }
                .distinctBy { it.id }.forEach { evictListRows(it.id) }
            accepted = rows.mapNotNull { row ->
                // Visibility is authoritative even when a content receipt has a newer list version.
                if (row.cleanupOnly) return@mapNotNull receiveList(row, mark)
                val newer = newerLists.find { it.id == row.id && it.occurrenceId == row.occurrenceId && it.version >= row.version }
                val base = newer ?: row
                val entries = newerEntries.filter { it.listId == row.id && it.occurrenceId == row.occurrenceId }
                fun merge(original: List<EntryResponse>) = original.map { entry -> entries.find { it.id == entry.id && it.version > entry.version } ?: entry }
                // Keep newly created entries until a read started after their receipt covers them.
                val missing = entries.filter { entry -> (base.propertyEntries + base.gotIt).none { it.id == entry.id } && entry.archivedAt == null }
                val merged = (merge(base.propertyEntries + base.gotIt) + missing).sortedWith(compareBy({ it.position }, { it.id }))
                receiveList(base.copy(propertyEntries = merged.filter { base.mode == ListMode.per_occurrence || it.checkedAt == null },
                    gotIt = if (base.mode == ListMode.persistent) merged.filter { it.checkedAt != null } else emptyList()), mark)
            }
        }
        return accepted
    }
    private fun receiveList(row: ListResponse, requestedAfter: Long?): ListResponse? {
        val accepted = visibleListEntries(acceptListVisibility(row, requestedAfter))
        putListRow(accepted)
        listReceipts.receive("${accepted.id}:${accepted.occurrenceId.orEmpty()}", accepted) { it.version }
        collaborationDeletions.remove("list" to accepted.id)
        return accepted
    }
    private fun redactedList(id: String): ListResponse? = q.listsById(id).executeAsList()
        .map { WireJson.decodeFromString<ListResponse>(it.payload) }.firstOrNull { it.redacted == true }
    private fun acceptListVisibility(row: ListResponse, requestedAfter: Long?): ListResponse {
        try { requireVisible("List", row.id, row.itemId, requestedAfter?.let { minOf(it, committingVisibility ?: it) } ?: committingVisibility) }
        catch (e: StaleVisibilityReply) { return redactedList(row.id) ?: throw e }
        if (row.cleanupOnly) {
            purgeListContent(row.id)
            if (row.redacted == true) evicted("List", row.id)
            return row.copy(occurrenceId = null)
        }
        if (redactedList(row.id) != null) purgeListContent(row.id)
        return row
    }
    /** Search never restores access. Both its request lease and the live UI revision fence its content. */
    suspend fun visibleSearchLists(fence: Fence, rows: List<ListResponse>): List<ListResponse> {
        var accepted = emptyList<ListResponse>()
        transaction(fence) {
            accepted = rows.filter { row ->
                try {
                    requireVisible("List", row.id, row.itemId)
                    row.redacted != true && redactedList(row.id) == null
                } catch (_: StaleVisibilityReply) { false }
            }
        }
        return accepted
    }
    /** Non-persisted results use the same receipt gate as SQL installs and the same live-view gate as editors. */
    suspend fun <T> retain(fence: Fence, value: T, resources: List<CacheResource>): RetainedCacheValue<T> {
        var accepted: RetainedCacheValue<T>? = null
        transaction(fence) {
            resources.forEach { requireVisible(it.kind, it.id) }
            accepted = RetainedCacheValue(value, visibilityRevision)
        }
        return requireNotNull(accepted)
    }
    private fun purgeListContent(id: String) {
        q.deleteList(id); q.evictEntry(id)
        listReceipts.keys.filter { listReceipts[it]?.second?.id == id }.forEach { listReceipts.remove(it) }
        entryReceipts.keys.filter { entryReceipts[it]?.second?.listId == id }.forEach { entryReceipts.remove(it) }
    }
    private fun evictListRows(id: String) {
        evicted("List", id)
        purgeListContent(id)
        val seq = ++receiptSeq
        collaborationDeletions["list" to id] = seq
    }
    suspend fun putEntry(fence: Fence, row: EntryResponse) = transaction(fence) {
        if (redactedList(row.listId) == null) { putEntryRow(row); entryReceipts.receive("${row.id}:${row.occurrenceId.orEmpty()}", row) { it.version } }
    }
    private fun putEntryRow(row: EntryResponse) {
        requireVisible("List", row.listId); requireVisible("Entry", row.id)
        if (redactedList(row.listId) != null) return
        val list = q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.find { it.id == row.listId && it.occurrenceId == row.occurrenceId } ?: return
        val previous = (list.propertyEntries + list.gotIt).find { it.id == row.id }
        if (previous != null && previous.version > row.version) return
        if (row.archivedAt != null) { evictEntryRows(row.id, row.listId); return }
        val rows = ((list.propertyEntries + list.gotIt).filter { it.id != row.id } + if (row.archivedAt == null) listOf(row) else emptyList()).sortedWith(compareBy({ it.position }, { it.id }))
        putListRow(list.copy(propertyEntries = rows.filter { list.mode == ListMode.per_occurrence || it.checkedAt == null },
            gotIt = if (list.mode == ListMode.persistent) rows.filter { it.checkedAt != null } else emptyList()))
    }
    private fun visibleListEntries(row: ListResponse): ListResponse {
        fun visible(entry: EntryResponse): Boolean = try {
            requireVisible("Entry", entry.id); entry.archivedAt == null
        } catch (_: StaleVisibilityReply) { false }
        return row.copy(propertyEntries = row.propertyEntries.filter(::visible), gotIt = row.gotIt.filter(::visible))
    }
    private fun evictEntryRows(id: String, listId: String) {
        evicted("Entry", id)
        q.deleteEntry(id)
        entryReceipts.keys.filter { entryReceipts[it]?.second?.id == id }.forEach { entryReceipts.remove(it) }
        q.listsById(listId).executeAsList().forEach { cached ->
            val row = WireJson.decodeFromString<ListResponse>(cached.payload)
            val retained = row.copy(propertyEntries = row.propertyEntries.filterNot { it.id == id }, gotIt = row.gotIt.filterNot { it.id == id })
            q.putList(row.id, row.occurrenceId.orEmpty(), row.itemId, null, WireJson.encodeToString(retained))
        }
    }
    private fun putListRow(row: ListResponse) {
        val row = visibleListEntries(row)
        val incoming = (row.propertyEntries + row.gotIt).mapTo(mutableSetOf()) { it.id }
        q.listsById(row.id).executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }
            .filter { it.occurrenceId == row.occurrenceId }.flatMap { it.propertyEntries + it.gotIt }
            .filter { it.id !in incoming }.forEach { evictEntryRows(it.id, row.id) }
        if (row.redacted != true) {
            requireVisible("List", row.id, row.itemId)
            row.occurrenceId?.let { requireVisible("ListScope", "${row.id}:$it"); requireVisible("Occurrence", it) }
            q.restoreVisibility("List", row.id)
        }
        q.putList(row.id, row.occurrenceId.orEmpty(), row.itemId, null, WireJson.encodeToString(row))
        q.evictEntryScope(row.id, row.occurrenceId.orEmpty())
        (row.propertyEntries + row.gotIt).forEach { q.restoreVisibility("Entry", it.id); q.putEntry(it.id, row.occurrenceId.orEmpty(), row.id, null, WireJson.encodeToString(it)) }
        trimListScopes()
    }
    private val activeListScopes = mutableMapOf<Pair<String, String>, Int>()
    /** Keep an editor's scope through LRU churn, never through ACL loss or occurrence expiry. */
    suspend fun <T> keepListScope(id: String, occurrenceId: String?, block: suspend () -> T): T {
        if (occurrenceId == null) return block()
        val key = id to occurrenceId
        mutex.withLock { activeListScopes[key] = (activeListScopes[key] ?: 0) + 1 }
        try { return block() }
        finally { withContext(NonCancellable) {
            mutex.withLock {
                val count = activeListScopes.getValue(key) - 1
                if (count == 0) activeListScopes.remove(key) else activeListScopes[key] = count
            }
        } }
    }
    private fun scopedListRows() = q.allLists().executeAsList().filter { it.occurrence_id.isNotEmpty() }
    /** Retain at most 32 scopes (or the active editors, if more). Templates do not count.
     * ORDER BY rowid / INSERT OR REPLACE keeps the most recently fetched inactive scopes. */
    private fun trimListScopes() {
        val scopes = scopedListRows()
        scopes.filterNot { (it.id to it.occurrence_id) in activeListScopes }
            .take((scopes.size - 32).coerceAtLeast(0)).forEach { evictListScopeRows(it.id, it.occurrence_id) }
    }
    private fun evictListScopeRows(id: String, occurrenceId: String) {
        q.deleteListScope(id, occurrenceId); q.evictEntryScope(id, occurrenceId)
        listReceipts.remove("$id:$occurrenceId")
        val keys = entryReceipts.filterValues { it.second.listId == id && it.second.occurrenceId == occurrenceId }.keys.toList()
        keys.forEach { entryReceipts.remove(it) }
    }
    suspend fun evictListScope(id: String, occurrenceId: String) = transaction { evicted("ListScope", "$id:$occurrenceId"); evictListScopeRows(id, occurrenceId) }
    suspend fun evictList(id: String) = transaction { evictListRows(id) }
    suspend fun evictComment(id: String) = transaction { evicted("Comment", id); q.deleteComment(id); evictCommentAttachments(id); collaborationDeletions["comment" to id] = ++receiptSeq }
    private fun evictCommentAttachments(id: String) {
        q.allAttachments().executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) }.filter { it.commentId == id }.forEach { q.deleteAttachment(it.id) }
    }
    suspend fun putComments(fence: Fence, rows: List<Comment>) = transaction(fence) { putCommentRows(rows) }
    private fun putCommentRows(rows: List<Comment>) {
        rows.forEach { requireVisible("Comment", it.id, it.itemId); q.restoreVisibility("Comment", it.id); q.putComment(it.id, it.itemId, null, WireJson.encodeToString(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    private fun putCommentHistory(itemId: String, history: CommentHistory) = q.putCommentHistory(itemId, history.nextCursor,
        if (history.started) 1 else 0, if (history.complete) 1 else 0, history.olderPages.toLong())
    suspend fun setCommentWindow(fence: Fence, itemId: String, olderPages: Int) = transaction(fence) {
        requireVisible("Item", itemId)
        putCommentHistory(itemId, (mutable.value.commentHistory[itemId] ?: CommentHistory()).copy(olderPages = olderPages.coerceAtLeast(0)))
    }
    suspend fun putCommentPage(fence: Fence, itemId: String, cursor: String?, page: CommentPage) = transaction(fence) {
        requireVisible("Item", itemId)
        require(page.items.all { it.itemId == itemId })
        if (cursor == null && page.nextCursor == null) replaceCommentRows(fence, itemId, page.items)
        else putCommentRows(page.items)
        val history = mutable.value.commentHistory[itemId] ?: CommentHistory()
        // A tail or out-of-order page is not proof that the gap before it has been fetched.
        if (cursor == null || (history.started && history.nextCursor == cursor)) {
            putCommentHistory(itemId, history.copy(started = true, nextCursor = page.nextCursor,
                complete = history.complete || page.nextCursor == null))
        }
    }
    suspend fun replaceComments(fence: Fence, itemId: String, rows: List<Comment>) = transaction(fence) {
        replaceCommentRows(fence, itemId, rows)
        putCommentHistory(itemId, (mutable.value.commentHistory[itemId] ?: CommentHistory()).copy(started = true, complete = true, nextCursor = null))
    }
    private fun replaceCommentRows(fence: Fence, itemId: String, rows: List<Comment>) {
        requireVisible("Item", itemId)
        val newer = commentReceipts.values.filter { it.first > (fence as Lease).visibility && it.second.itemId == itemId }.map { it.second }
        val accepted = rows.associateByTo(mutableMapOf()) { it.id }
        newer.forEach { row -> if ((accepted[row.id]?.version ?: 0) <= row.version) accepted[row.id] = row }
        q.allComments().executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) }.filter { it.itemId == itemId && it.id !in accepted }.forEach {
            evicted("Comment", it.id); collaborationDeletions["comment" to it.id] = ++receiptSeq; evictCommentAttachments(it.id)
        }
        q.evictComment(itemId); accepted.values.forEach { requireVisible("Comment", it.id, it.itemId); q.restoreVisibility("Comment", it.id); q.putComment(it.id, it.itemId, null, WireJson.encodeToString(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    suspend fun putAttachment(fence: Fence, row: Attachment) = transaction(fence) { requireVisible("Attachment", row.id, row.itemId); row.commentId?.let { requireVisible("Comment", it) }; q.restoreVisibility("Attachment", row.id); q.putAttachment(row.id, row.itemId, null, WireJson.encodeToString(row)); attachmentReceipts.receive(row.id, row) { it.version }; collaborationDeletions.remove("attachment" to row.id) }
    suspend fun evictAttachment(id: String) = withContext(NonCancellable) {
        transaction { evicted("Attachment", id); q.deleteAttachment(id); collaborationDeletions["attachment" to id] = ++receiptSeq }
    }
    /** Serialize private viewer creation with metadata eviction, even without a trust controller in tests. */
    internal suspend fun withLiveAttachment(row: Attachment, action: suspend () -> Unit) = withContext(Dispatchers.Default) {
        mutex.withLock {
            val fence = sessionCommitMutex
            fence?.lock()
            try {
                currentCoroutineContext().ensureActive()
                val lease = currentCoroutineContext()[Lease]
                requireVisible("Attachment", row.id, row.itemId, lease?.visibility)
                row.commentId?.let { requireVisible("Comment", it, receivedAt = lease?.visibility) }
                if (lease != null && (lease.epoch != sessionEpoch() || lease.storage != storageGeneration))
                    throw CancellationException("Household session or cache changed")
                require(mutable.value.attachments.any { it.id == row.id && it.version == row.version && it.deletedAt == null }) { "Attachment is no longer visible" }
                attachmentCleanup.drain()
                action()
            } finally { fence?.unlock() }
        }
    }
    suspend fun replaceMembers(fence: Fence, rows: List<MemberRead>, receivedAfter: Long? = null) = transaction(fence) {
        val incoming = rows.associateBy { it.id }
        q.clearMembers(); rows.forEach { q.putMember(it.id, null, null, WireJson.encodeToString(it)) }
        receivedAfter?.let { mark ->
            for ((seq, row) in memberReceipts.values) {
                val installed = incoming[row.id] ?: continue
                if (seq > mark && row.version > installed.version) q.putMember(row.id, null, null, WireJson.encodeToString(row))
            }
        }
        // A lower authoritative version starts a restored history for this member. Do not stamp an
        // old high-water receipt with a later write's sequence, resurrecting pre-restore data.
        // Equal/newer collection rows keep receipts needed by an older in-flight snapshot (n-2).
        memberReceipts.entries.removeAll { (id, receipt) ->
            receivedAfter != null && receipt.first <= receivedAfter &&
                (incoming[id] == null || incoming.getValue(id).version < receipt.second.version)
        }
    }
    suspend fun putMember(fence: Fence, row: MemberRead) = transaction(fence) {
        val installed = q.allMembers().executeAsList().find { it.id == row.id }?.let { WireJson.decodeFromString<MemberRead>(it.payload) }
        if (installed == null || installed.version <= row.version) q.putMember(row.id, null, null, WireJson.encodeToString(row))
        memberReceipts.receive(row.id, row) { it.version }
    }
    suspend fun putSettings(fence: Fence, row: SettingsRead) = transaction(fence) {
        val installed = q.allSettingss().executeAsList().firstOrNull()?.let { WireJson.decodeFromString<SettingsRead>(it.payload) }
        if (installed == null || installed.memberId != row.memberId || installed.version <= row.version) {
            q.clearSettingss(); q.putSettings(row.memberId, null, null, WireJson.encodeToString(row))
        }
        val previous = settingsReceipt?.second
        settingsReceipt = ++receiptSeq to
            (if (previous != null && previous.memberId == row.memberId && previous.version > row.version) previous else row)
    }
    suspend fun replaceBoard(fence: Fence, rows: List<ItemRead>) = transaction(fence) { replaceBoardRows(rows) }
    /** Agenda refetch: replaces in place only what [window] authoritatively covered. */
    suspend fun replaceAgenda(fence: Fence, window: AgendaWindow) = transaction(fence) {
        mutable.value.let { replaceAgendaRows(window, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** Replaces every cached agenda row (test seeding; production refetches use [AgendaWindow]). */
    internal suspend fun replaceAgenda(fence: Fence, rows: List<ItemRead>) = transaction(fence) {
        q.clearAgendas()
        rows.forEach { row -> row.agenda?.let { q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, mutable.value.members), WireJson.encodeToString(row), it.occurrenceId) } }
        cleanRows(now())
    }
    /** Write follow-up (R59): board and agenda switch together, in place, never cleared ahead of a fetch. */
    suspend fun replaceProjections(fence: Fence, projections: Projections) = transaction(fence) {
        replaceBoardRows(projections.board)
        mutable.value.let { replaceAgendaRows(projections.agenda, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** A windowed `/v1/items/{id}/occurrences` answer never contains undated occurrences, nor anything outside
     * [from, to): only returned rows and cached rows the window provably covered (now gone) are replaced (I-2). The
     * window is required: there is no "replace everything" default that could drop undated rows (N-3). */
    suspend fun putOccurrences(fence: Fence, itemId: String, rows: List<OccurrenceResponse>, from: Instant, to: Instant,
                               includeUndated: Boolean = false, receivedAfter: Long? = null) = transaction(fence) {
        val returned = rows.mapTo(HashSet()) { it.id }
        // m-7: whether dated-window or undated coverage, a missing row is deleted only if nothing newer than the request
        // (an own-write response or a snapshot install) observed it.
        mutable.value.occurrences.filter { it.itemId == itemId && it.id !in returned &&
            (provablyInWindow(it, from, to) || (includeUndated && it.startsAt == null && it.dueAt == null && it.localStart == null && it.localDue == null && it.dueDate == null)) &&
            (receivedAfter?.let { mark -> (occurrenceObservations[it.id] ?: 0L) <= mark } ?: (it.id !in occurrenceReceipts)) }
            .forEach { evictOccurrenceRows(it.id) }
        val items = mutable.value.items.associateBy { it.id }
        val occurrences = mutable.value.occurrences.associateBy { it.id }
        rows.forEach { putOccurrenceRow(it, items, occurrences) }
        cleanRows(now())
    }
    suspend fun putItem(fence: Fence, response: ItemResponse) = putItems(fence, listOf(response))
    /** One transaction for a whole page of item reads/writes. */
    suspend fun putItems(fence: Fence, responses: List<ItemResponse>) = transaction(fence) {
        val existing = mutable.value.items.associateBy { it.id }
        responses.forEach { putItemRow(it, existing[it.id]); itemReceipts.receive(it.id, it) { row -> row.version } }
    }
    private fun putItemRow(response: ItemResponse, existing: ItemRead?) {
        requireVisible("Item", response.id); q.restoreVisibility("Item", response.id)
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
    suspend fun putOccurrence(fence: Fence, response: OccurrenceResponse) = transaction(fence) {
        putOccurrenceRow(response, mutable.value.items.associateBy { it.id }, mutable.value.occurrences.associateBy { it.id })
        occurrenceReceipts.receive(response.id, response) { it.version }
    }
    private fun putOccurrenceRow(response: OccurrenceResponse, items: Map<String, ItemRead>, occurrences: Map<String, OccurrenceRead>,
                                 zone: TimeZone = ownZone()) {
        requireVisible("Occurrence", response.id, response.itemId); q.restoreVisibility("Occurrence", response.id)
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
        if (!closed) { closed = true; closeDriver() }
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
