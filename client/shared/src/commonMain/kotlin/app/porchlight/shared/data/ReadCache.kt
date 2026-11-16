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
    val sources: List<SourceSnapshot> = emptyList(),
    val agendaWindows: Set<ItemAgendaWindow> = emptySet(),
    val boardScope: String = "household",
) {
    // Process-local draft fences are not part of payload equality (nor persisted cache data).
    internal var resourceVisibility: Map<CacheResource, Any> = emptyMap()
    internal var identityVisibility: Any? = null
    internal var quarantinedResources: Map<CacheResource, Any> = emptyMap()
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
const val ATTACHMENT_CLEANUP_NOTICE = "Private file cleanup failed. Sync is paused. Check storage permissions and free space, then clear the local cache to recover."

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
    private val databaseDispatcher = Dispatchers.IO.limitedParallelism(1)
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
    internal var attachmentNamespace = 0L; private set
    private val mutableCleanupRetryAt = MutableStateFlow<Instant?>(null)
    val cleanupRetryAt: StateFlow<Instant?> = mutableCleanupRetryAt.asStateFlow()
    private val attachmentCleanup = AttachmentCleanup(attachmentStore, now) { db }
    private suspend fun drainAttachmentCleanup(): SyncState? = try {
        attachmentCleanup.drain()
    } finally {
        val failure = q.getCleanupFailure().executeAsOneOrNull()
        mutableCleanupRetryAt.value = failure?.let { Instant.fromEpochMilliseconds(it.retry_at) }
        if (failure != null) mutableStorageNotice.value = ATTACHMENT_CLEANUP_NOTICE
        else if (mutableStorageNotice.value == ATTACHMENT_CLEANUP_NOTICE) mutableStorageNotice.value = null
    }
    private suspend fun ensureAttachmentNamespace() {
        val stored = q.getFileNamespace().executeAsOneOrNull()
        if (stored != null) { attachmentNamespace = stored; return }
        // Upgrade/reset cannot attribute old process-epoch files. Purge before admitting any private files.
        db.transaction { q.putAttachmentCleanup("clear", "namespace") }
        drainAttachmentCleanup()
        rotateAttachmentNamespace()
    }
    private fun rotateAttachmentNamespace() {
        attachmentNamespace = kotlin.random.Random.nextLong(1, Long.MAX_VALUE)
        q.putFileNamespace(attachmentNamespace)
    }
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
    internal var sessionRevision: () -> Long = { 0L }
    internal var writeOrigin: () -> WriteOrigin = { WriteOrigin(sessionEpoch(), null, sessionRevision()) }
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
    private var itemRedactionPending = false
    private fun evicted(kind: String, id: String) {
        q.putVisibility(kind, id, ++receiptSeq)
        visibilityRevision = receiptSeq
        resourceVisibility = resourceVisibility - CacheResource(kind, id)
        quarantinedResources = quarantinedResources - CacheResource(kind, id)
        if (kind == "Item") {
            evicted("Inbox", "self"); q.clearNotifications()
            itemRedactionPending = true
        }
    }
    /** Decorations have a different owner from their content. Their referenced resources must
     * still be visible, even when the owning row itself passes its request-start fence. */
    private fun referenceHidden(kind: String, id: String): Boolean {
        if (committingVisibility?.let { it < visibilityFloor } == true) return true
        val state = q.referenceVisibility(kind, id).executeAsOneOrNull() ?: return false
        return state.evicted != 0L || committingVisibility?.let { state.generation > it } == true
    }
    private fun sanitizedPayload(payload: String): String {
        val raw = WireJson.parseToJsonElement(payload) as? JsonObject ?: return payload
        val conflicts = raw["conflicts"] as? JsonArray
        val safeConflicts = conflicts?.map { element ->
            val conflict = element.jsonObject
            fun hidden(field: String, kind: String) = conflict[field]?.jsonPrimitive?.contentOrNull?.let { referenceHidden(kind, it) } == true
            if (!hidden("other_item_id", "Item") && !hidden("other_occurrence_id", "Occurrence")) element
            else JsonObject(conflict + mapOf("other_item_id" to JsonNull, "other_occurrence_id" to JsonNull, "other_title" to JsonPrimitive("Busy")))
        }?.let(::JsonArray)
        if (safeConflicts == conflicts || safeConflicts == null) return payload
        return JsonObject(raw + ("conflicts" to safeConflicts)).toString()
    }
    private inline fun <reified T> payload(value: T): String {
        val encoded = WireJson.encodeToString(value)
        return if (value is ItemRead || value is ItemResponse || value is OccurrenceResponse || value is SuggestionResponse)
            sanitizedPayload(encoded) else encoded
    }
    /** One eviction transaction redacts every retained copy before publication. Inbox summaries
     * may coalesce hidden sources without exposing their IDs, so invalidate the entire inbox. */
    private fun redactItemReferences() {
        q.allItems().executeAsList().forEach { row -> val safe = sanitizedPayload(row.payload); if (safe != row.payload) q.putItem(row.id, row.item_id, row.expires_at, safe) }
        q.allBoards().executeAsList().forEach { row -> val safe = sanitizedPayload(row.payload); if (safe != row.payload) q.putBoard(row.id, row.item_id, row.expires_at, safe) }
        q.allAgendas().executeAsList().forEach { row -> val safe = sanitizedPayload(row.payload); if (safe != row.payload) q.putAgenda(row.id, row.item_id, row.expires_at, safe, row.occurrence_id) }
        q.allWrites().executeAsList().forEach { row -> val safe = sanitizedPayload(row.payload); if (safe != row.payload) q.putWrite(row.id, row.item_id, row.expires_at, safe) }
        q.allPlanning().executeAsList().filter { it.kind == "suggestion" }.forEach { row -> val safe = sanitizedPayload(row.payload); if (safe != row.payload) q.putPlanning(row.kind, row.id, row.item_id, safe) }
        q.allLists().executeAsList().filter { it.item_id?.let { parent -> referenceHidden("Item", parent) } == true }.map { it.id }.distinct().forEach { evictListRows(it) }
    }
    private fun requireVisible(kind: String, id: String, itemId: String? = null, receivedAt: Long? = committingVisibility) {
        val mark = receivedAt ?: return
        if (mark < visibilityFloor || (q.getVisibility(kind, id).executeAsOneOrNull() ?: 0) > mark ||
            (itemId != null && (q.getVisibility("Item", itemId).executeAsOneOrNull() ?: 0) > mark))
            throw StaleVisibilityReply()
    }
    private class StaleCollectionReply : Exception("Collection changed during read")
    private class CollectionRead(val started: Long) : AbstractCoroutineContextElement(Key) {
        companion object Key : CoroutineContext.Key<CollectionRead>
        val committed = mutableMapOf<String, Long>()
    }
    private val collectionRepair = Mutex()
    private var changedLists = mutableSetOf<String>()
    private var collectionGeneration = 0L
    private val collectionCommits = mutableMapOf<String, Long>()
    private val snapshotCollections = setOf("items", "occurrences", "board", "agenda", "comments", "lists", "inbox", "members", "settings", "prefs", "planning", "feeds")

    /** All repository reads (including screen/worker entry) share commit ordering with targeted/full reads.
     * Reject before SQL writes, then repair only this read's scope with a genuinely new request-start fence. */
    internal suspend fun <T> orderedRead(block: suspend (Fence) -> T): T = inSession { fence ->
        if (currentCoroutineContext()[CollectionRead] != null) return@inSession block(fence)
        suspend fun attempt(admitted: Fence): T {
            val started = withContext(databaseDispatcher) { mutex.withLock { collectionGeneration } }
            return withContext(CollectionRead(started)) { block(admitted) }
        }
        suspend fun repair(): T {
            while (true) {
                // A collection repair is not permission to renew an old caller's privacy lease.
                if (mutable.value.visibilityRevision > (fence as Lease).visibility) throw StaleVisibilityReply()
                try { return withFreshVisibility(fence) { attempt(it) } }
                catch (_: StaleCollectionReply) { currentCoroutineContext().ensureActive() }
            }
        }
        try { attempt(fence) }
        catch (_: StaleCollectionReply) { collectionRepair.withLock { repair() } }
    }

    sealed interface Fence
    private class Lease(val owner: ReadCache, val epoch: Long, val visibility: Long, val storage: Long, val revision: Long, val currentRevision: () -> Long) : AbstractCoroutineContextElement(Key), Fence {
        companion object Key : CoroutineContext.Key<Lease>
    }
    /** The only request-start API. Nested reads inherit the original fence, never a receipt-time fence. */
    suspend fun <T> inSession(block: suspend (Fence) -> T): T {
        currentCoroutineContext()[Lease]?.takeIf { it.owner === this }?.let { return block(it) }
        val epoch = sessionEpoch()
        val readSession = currentCoroutineContext()[ReadSession]
        val currentRevision = readSession?.currentRevision ?: sessionRevision
        val revision = readSession?.revision ?: currentRevision()
        val lease = withContext(databaseDispatcher) {
            mutex.withLock {
                if (closed) throw CancellationException("Read cache closed")
                if (ownerBindingRequired && !ownerReady) throw CacheOwnerUnavailable()
                db // open/seed the monotonic counter once, never scan history per request
                Lease(this@ReadCache, epoch, receiptSeq, storageGeneration, revision, currentRevision)
            }
        }
        return withContext(lease) { block(lease) }
    }
    internal var freshVisibilityCaptured: suspend () -> Unit = {}
    /** Capture and publish a scheduled read's fence under admission's lock, released before any network work. */
    internal suspend fun <T> withFreshVisibility(fence: Fence, activationLock: Mutex? = null,
        onStarted: (Long) -> Unit = {}, block: suspend (Fence) -> T): T {
        val admitted = fence as Lease
        check(admitted.owner === this && currentCoroutineContext()[Lease] === admitted)
        suspend fun activate(): Lease {
            val fresh = withContext(databaseDispatcher) {
                mutex.withLock {
                    if (closed || admitted.epoch != sessionEpoch() || admitted.storage != storageGeneration || admitted.revision != admitted.currentRevision())
                        throw CancellationException("Household cache changed")
                    if (ownerBindingRequired && !ownerReady) throw CacheOwnerUnavailable()
                    Lease(this@ReadCache, admitted.epoch, receiptSeq, admitted.storage, admitted.revision, admitted.currentRevision)
                }
            }
            freshVisibilityCaptured()
            onStarted(fresh.visibility)
            return fresh
        }
        val fresh = if (activationLock == null) activate() else activationLock.withLock { activate() }
        return withContext(fresh) { block(fresh) }
    }
    internal suspend fun refreshKey(fence: Fence): Triple<Long, Long, Long> {
        val lease = fence as Lease
        check(lease.owner === this && currentCoroutineContext()[Lease] === lease)
        return Triple(lease.epoch, lease.storage, lease.visibility)
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
    private var quarantinedResources: Map<CacheResource, Any> = emptyMap()
    private fun publish(next: CacheView, resetVisibility: Boolean = false, storageRecovery: Boolean = false) {
        if (storageRecovery) {
            visibilityRevision = ++receiptSeq
            quarantinedResources = quarantinedResources + resourceVisibility
            resourceVisibility = emptyMap()
        }
        if (resetVisibility) {
            visibilityRevision = ++receiptSeq; resourceVisibility = emptyMap(); identityVisibility = Any()
            quarantinedResources = emptyMap()
        }
        val resources = buildSet {
            (next.items + next.board + next.agenda).forEach { add(CacheResource("Item", it.id)) }
            next.itemWrites.keys.forEach { add(CacheResource("Item", it)) }
            next.occurrences.forEach { add(CacheResource("Occurrence", it.id)) }
            next.comments.filter { it.deletedAt == null }.forEach { add(CacheResource("Comment", it.id)) }
            next.lists.filter { it.redacted != true }.forEach { add(CacheResource("List", it.id)) }
        }
        // Only a new authorised install can release a quarantined editor. A completed snapshot or
        // explicit denial retires missing resources; subsequent access cannot resurrect those drafts.
        quarantinedResources = quarantinedResources.filterKeys { resource ->
            !(resource.kind == "Item" && resource.id in next.evictedItemIds) &&
                !(resource.kind == "List" && resource.id in next.evictedListIds)
        }
        resourceVisibility = resources.associateWith { resourceVisibility[it] ?: quarantinedResources[it] ?: Any() }
        quarantinedResources = if (next.sync != null) emptyMap() else quarantinedResources - resources
        mutable.value = next.copy(visibilityRevision = visibilityRevision).also {
            it.resourceVisibility = resourceVisibility; it.identityVisibility = identityVisibility
            it.quarantinedResources = quarantinedResources
        }
        mutableSync.value = next.sync
    }
    private class InvalidCachePayload(cause: Exception) : Exception("Disposable cache payload is invalid", cause)
    private fun load(): CacheView = try {
        val planning = q.allPlanning().executeAsList()
        val scopes = q.allProjectionScopes().executeAsList()
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
        agendaWindows = scopes.filter { it.kind == "agenda" }.mapTo(mutableSetOf()) { WireJson.decodeFromString<ItemAgendaWindow>(it.payload) },
        boardScope = scopes.singleOrNull { it.kind == "board" }?.payload ?: "household",
        sources = planning.filter { it.kind == "feed" }.map { WireJson.decodeFromString<SourceSnapshot>(it.payload) },
        evictedListIds = q.evictedVisibility("List").executeAsList().toSet(),
        evictedItemIds = q.evictedVisibility("Item").executeAsList().toSet(),
        commentHistory = q.allCommentHistories().executeAsList().associate { it.item_id to
            CommentHistory(it.next_cursor, it.started != 0L, it.complete != 0L, it.older_pages.toInt()) },
        )
    } catch (e: IllegalArgumentException) { throw InvalidCachePayload(e) }
    /** [fenced] = false: the caller's inherited session lease does not apply (identity clears must always land). */
    private suspend fun transaction(fence: Fence, publication: (() -> CacheView)? = null, retryCleanup: Boolean = false,
                                    collections: Set<String> = emptySet(), provisional: Boolean = false, block: () -> Unit) {
        val lease = fence as Lease
        check(lease.owner === this && currentCoroutineContext()[Lease] === lease) { "Use the request-start fence inside inSession" }
        transaction(publication = publication, retryCleanup = retryCleanup, collections = collections, provisional = provisional, block = block)
    }
    // Only lifecycle/removal operations may use the unfenced transaction overload.
    private suspend fun transaction(fenced: Boolean = true, recover: Boolean = false, binding: CacheOwner? = null, cursorOnly: Boolean = false, localReset: Boolean = false, publication: (() -> CacheView)? = null, retryCleanup: Boolean = false, collections: Set<String> = emptySet(), provisional: Boolean = false, block: () -> Unit) = withContext(databaseDispatcher) { mutex.withLock {
        val identityClear = !fenced && binding == null && !localReset
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
            if (lease != null && (lease.epoch != sessionEpoch() || lease.storage != storageGeneration || lease.revision != lease.currentRevision()))
                throw CancellationException("Household session or cache changed")
        }
        val fence = sessionCommitMutex
        fence?.lock()
        try {
            checkLease()
            if (identityClear) { expectedOwner = null; identityClearPending = true; hideOwner() }
            committingVisibility = lease?.visibility
            var cleanupRequired = recover || identityClear || retryCleanup
            while (true) {
                try {
                    val database = db
                    if (ownerBindingRequired && !identityClear && !localReset) reconcileOwner()
                    if (!identityClear && !localReset) ensureAttachmentNamespace()
                    checkLease()
                    // A stream position is never durable while any committed private purge is pending.
                    if (cursorOnly) withContext(NonCancellable) { drainAttachmentCleanup() }
                    val priorVisibility = visibilityRevision
                    val priorFloor = visibilityFloor
                    val priorResources = resourceVisibility
                    val priorQuarantine = quarantinedResources
                    var reset = false
                    itemRedactionPending = false
                    changedLists = mutableSetOf()
                    val next = try { database.transactionWithResult<CacheView> {
                        // Owner and cursor checks share the operation's SQLite snapshot with ALL payload reads.
                        // A separate probe could consume an external reset without notifying the live stream.
                        if (ownerBindingRequired && !identityClear && !localReset && expectedOwner != null) {
                            val owner = q.getCacheOwner().executeAsOneOrNull()
                            if (owner?.ca_fingerprint != expectedOwner?.caFingerprint || owner?.device_id != expectedOwner?.deviceId || owner?.member_id != expectedOwner?.memberId) {
                                ownerReady = false
                                throw CacheOwnerUnavailable()
                            }
                        }
                        val previousSync = q.getSync().executeAsOneOrNull()
                        reset = !localReset && mutableSync.value != null && previousSync == null
                        if (!reset) {
                            if (ownerBindingRequired && expectedOwner == null && !localReset) {
                                // Initialized ownerless is empty, never permission to load anonymous disk rows.
                                clearRows(); q.clearCacheOwner()
                            } else {
                                context[CollectionRead]?.let { read ->
                                    if (collections.any { name -> (collectionCommits[name] ?: 0L).let { it > read.started && it != read.committed[name] } })
                                        throw StaleCollectionReply()
                                }
                                block()
                            }
                            if (itemRedactionPending) redactItemReferences()
                            checkLease(); trimVisibility()
                        }
                        // Cursor-only work shares every admission/reset/commit fence, but never reads payloads.
                        if (cursorOnly && !reset) mutable.value.copy(sync = q.getSync().executeAsOneOrNull()?.let {
                            SyncState(it.cursor, Instant.fromEpochMilliseconds(it.last_full_sync))
                        }) else {
                            val next = if (!reset && publication != null && !(ownerBindingRequired && expectedOwner == null)) publication() else load()
                            if (!reset) cleanupRequired = attachmentCleanup.transition(mutable.value, next, attachmentNamespace, clearIdentity = identityClear) || cleanupRequired
                            val deferred = !reset && next.sync != null &&
                                (next.sync.cursor != previousSync?.cursor || next.sync.lastFullSync.toEpochMilliseconds() != previousSync?.last_full_sync) &&
                                q.allAttachmentCleanup().executeAsList().isNotEmpty()
                            if (deferred) {
                                requireNotNull(next.sync).let { q.putPendingSync(it.cursor, it.lastFullSync.toEpochMilliseconds()) }
                                if (previousSync == null) q.clearSync() else q.putSync(previousSync.cursor, previousSync.last_full_sync)
                                next.copy(sync = previousSync?.let { SyncState(it.cursor, Instant.fromEpochMilliseconds(it.last_full_sync)) })
                            } else next
                        }
                    } } catch (e: Exception) {
                        visibilityRevision = priorVisibility; visibilityFloor = priorFloor; resourceVisibility = priorResources
                        quarantinedResources = priorQuarantine
                        throw e
                    }
                    if (reset) {
                        discardPublished(next)
                        withContext(NonCancellable) { drainAttachmentCleanup() }
                        checkLease() // The pre-reset operation cannot refill the cache with an old reply.
                        continue
                    }
                    if (cursorOnly) { mutableSync.value = next.sync; break }
                    // Mutation receipts are provisional, not authoritative replacement coverage. Older full
                    // reads still merge those receipts; a repository GET of the same shape is an ordered install.
                    if (collections.isNotEmpty() && (!provisional || context[CollectionRead] != null)) {
                        val generation = ++collectionGeneration
                        collections.forEach { name ->
                            collectionCommits[name] = generation
                            context[CollectionRead]?.committed?.set(name, generation)
                        }
                    }
                    if (localReset) {
                        storageGeneration++
                        mutableStorageRevision.value = storageGeneration
                    }
                    publish(next, resetVisibility = identityClear, storageRecovery = localReset)
                    break
                } catch (e: CancellationException) { throw e }
                catch (e: Exception) {
                    if (ownerBindingRequired && !ownerReady) {
                        hideOwner()
                        if (mutableCleanupRetryAt.value == null) mutableStorageNotice.value = CACHE_STORAGE_NOTICE
                    }
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
            if (cleanupRequired) withContext(NonCancellable) {
                // Publish only the cursor returned by its promotion transaction. An unrelated
                // out-of-transaction cursor probe could consume an external reset without signalling it.
                drainAttachmentCleanup()?.let { completed -> publish(mutable.value.copy(sync = completed)) }
            }
            if (localReset) db.transaction { rotateAttachmentNamespace() }
            if (identityClear) {
                db.transaction { rotateAttachmentNamespace() }
                ownerlessInitialized = true
                identityClearPending = false
            }
            if (q.getCleanupFailure().executeAsOneOrNull() == null) mutableStorageNotice.value = null
        } catch (e: Exception) {
            if (ownerBindingRequired && !ownerReady) {
                hideOwner()
                if (mutableCleanupRetryAt.value == null) mutableStorageNotice.value = CACHE_STORAGE_NOTICE
            }
            throw e
        } finally { committingVisibility = null; fence?.unlock() }
    } }
    private fun discardPublished(next: CacheView = CacheView()) {
        storageGeneration++
        clearReceipts(); occurrenceObservations.clear()
        attachmentCleanup.clearIdentity()
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
            withContext(NonCancellable) { drainAttachmentCleanup() }
            db.transaction { rotateAttachmentNamespace() }
        }
        if (clearRequired || stored?.member_id != owner.memberId)
            db.transaction { q.putCacheOwner(owner.caFingerprint, owner.memberId, owner.deviceId) }
        identityClearPending = false
        ownerReady = true
    }
    private inline fun <reified T> stage(kind: String, id: String, itemId: String?, expiry: Long?, value: T) {
        if (value !is ListResponse || value.redacted != true) {
            if (kind == "Occurrence") requireVisible("OccurrenceRow", id)
            requireVisible(kind, id, if (kind == "Entry") null else itemId)
            if (kind == "Entry" && itemId != null) requireVisible("List", itemId)
            q.restoreVisibility(kind, id)
        }
        q.stage(kind, id, itemId, expiry, payload(value), (value as? ScheduledReminderRead)?.occurrenceId)
    }

    /** Authoritative switch (R59): entities, cursor and — when given — the board/agenda [projections] fetched for this
     * snapshot commit in ONE transaction. Projection rows of items that are no longer visible disappear (snapshot
     * difference); fetched projections replace theirs in place. Throwing anywhere rolls back every table and cursor. */
    suspend fun install(fence: Fence, snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null) =
        installStaged(fence, snapshot, at, projections)
    /** One fenced scoped replacement. Coverage is per page, never a household deletion proof or stream cursor.
     * The caller serializes reads for a scope and re-dirties it for events arriving during the fetch. */
    suspend fun installItemBundle(fence: Fence, coverage: ItemBundleCoverage, rows: ItemBundleRows) {
        require(rows.pages.isNotEmpty() && coverage.pages == rows.pages.map { it.coverage })
        val id = coverage.pages.first().itemId
        require(coverage.pages.all { it.itemId == id })
        var touchedLists = emptySet<String>()
        transaction(fence, publication = { itemBundleView(id, touchedLists + changedLists) }, retryCleanup = true,
            collections = buildSet {
                if (coverage.pages.any { it.itemSections }) addAll(listOf("items", "occurrences", "board"))
                if (coverage.pages.any { it.agenda.isNotEmpty() }) add("agenda")
                if (coverage.pages.any { it.comments }) add("comments")
                if (coverage.pages.any { it.collaboration }) add("lists")
                if (coverage.pages.any { it.suggestions }) add("planning")
            }) {
            requireVisible("Item", id)
            val pages = rows.pages
            val intervals = bundleIntervals(pages)
            val canonical = pages.filter { it.coverage.itemSections }
            require(canonical.size <= 1)
            val initial = canonical.singleOrNull()
            val item = initial?.items?.singleOrNull() ?: mutable.value.items.find { it.id == id }
            require(initial == null || initial.items?.singleOrNull()?.id == id)
            require(pages.all { page ->
                page.items.orEmpty().all { it.id == id } && page.occurrences.orEmpty().all { it.itemId == id } &&
                page.board.orEmpty().all { it.id == id } && page.agenda.orEmpty().flatMap { it.items }.all { it.id == id } &&
                page.comments.orEmpty().all { it.itemId == id } && page.attachments.orEmpty().all { it.itemId == id } &&
                page.scheduledReminders.orEmpty().all { it.itemId == id } && page.suggestions.orEmpty().all { it.itemId == id }
            })
            // Validate before omission fences are written; a late child reply fails the entire bundle atomically.
            pages.forEach { page ->
                page.occurrences.orEmpty().forEach { requireOccurrenceRowVisible(it.id, id) }
                (page.board.orEmpty() + page.agenda.orEmpty().flatMap { it.items }).forEach { row ->
                    row.occurrence?.let { requireVisible("Occurrence", it.id, id) }
                    row.agenda?.let { requireVisible("Occurrence", it.occurrenceId, id) }
                }
                page.comments.orEmpty().forEach { requireVisible("Comment", it.id, id) }
                page.attachments.orEmpty().forEach { row -> requireVisible("Attachment", row.id, id); row.commentId?.let { requireVisible("Comment", it) } }
                page.lists.orEmpty().forEach { row -> requireVisible("List", row.id, row.itemId); (row.propertyEntries + row.gotIt).forEach { requireVisible("Entry", it.id) } }
                page.suggestions.orEmpty().forEach { requireSuggestionVisible(it) }
            }
            val oldLists = mutable.value.lists
            touchedLists = (pages.flatMap { it.lists.orEmpty() }.map { it.id } + oldLists.filter { it.itemId == id }.map { it.id }).toSet()
            initial?.let { page ->
                val c = page.coverage
                val incoming = requireNotNull(page.occurrences).mapTo(mutableSetOf()) { it.id }
                val removed = mutable.value.occurrences.filter { it.itemId == id && it.id !in incoming &&
                    bundleOccurrenceCovered(it, mutable.value.items.find { i -> i.id == id }, c.occurrences, c.undatedOccurrences) }
                // Snapshot-window absence is not definitive deletion. Each dependent collection
                // has independent coverage (including retained agendas and occurrence list checks).
                removed.forEach { removeScopedOccurrence(it.id) }
                requireNotNull(item)
                q.restoreVisibility("Item", id); q.putItem(id, id, null, payload(item))
                q.evictWrite(id); itemReceipts.remove(id); itemDeletions.remove(id); visibilityDeletions.remove(id)
                page.occurrences.forEach { row ->
                    q.restoreVisibility("Occurrence", row.id)
                    q.putOccurrence(row.id, id, expiry(row, ownZone(), item), payload(row))
                    occurrenceObservations[row.id] = ++receiptSeq
                }
                val reminders = requireNotNull(page.scheduledReminders)
                require(reminders.all { c.reminderMemberId == null || it.memberId == c.reminderMemberId })
                val reminderIds = reminders.mapTo(mutableSetOf()) { it.id }
                mutable.value.reminders.filter { it.itemId == id && it.id !in reminderIds &&
                    (c.reminderMemberId == null || it.memberId == c.reminderMemberId) &&
                    it.fireAt >= c.scheduledReminders.from && it.fireAt < c.scheduledReminders.to }.forEach { q.deleteReminder(it.id) }
                reminders.forEach { row -> requireVisible("Occurrence", row.occurrenceId, id)
                    q.putReminder(row.id, id, row.fireAt.toEpochMilliseconds(), payload(row), row.occurrenceId) }
                require(c.boardScope == "household")
                q.evictBoard(id)
                val member = mutable.value.settings?.memberId
                requireNotNull(page.board).filter { row -> mutable.value.boardScope == "household" ||
                    row.ownerId == member || row.subjectMemberId == member || member in row.assignees
                }.forEach { row -> row.occurrence?.let { q.putBoard(it.id, id, null, payload(row)) } }
            }
            pages.filter { it.coverage.suggestions }.forEach { page ->
                mutable.value.suggestions.filter { it.itemId == id }.forEach { q.removePlanning("suggestion", it.id) }
                requireNotNull(page.suggestions).forEach { q.putPlanning("suggestion", it.id, id, payload(it)) }
            }
            for (interval in intervals.agenda) {
                val scope = interval.scope
                val returned = interval.rows.mapNotNull { it.agenda?.occurrenceId }.toSet()
                val zone = scope.bundleZone()
                q.itemAgendas(id).executeAsList().forEach { cached ->
                    val a = WireJson.decodeFromString<ItemRead>(cached.payload).agenda ?: return@forEach
                    if (a.memberId != scope.memberId || a.occurrenceId in returned) return@forEach
                    val key = a.bundleKey(zone) ?: return@forEach
                    if (bundleAgendaCovered(a, scope, zone) && (interval.after == null || key > interval.after) &&
                        (scope.complete || interval.through?.let { key <= it } == true)) q.deleteAgenda(cached.id)
                }
                interval.rows.forEach { row -> row.agenda?.let { q.putAgenda(it.memberId + ":" + it.occurrenceId, id,
                    agendaExpiry(row, mutable.value.members), payload(row), it.occurrenceId) } }
            }
            for (interval in intervals.comments) {
                val returned = interval.rows.mapTo(mutableSetOf()) { it.id }
                q.itemComments(id).executeAsList().filter { it.id !in returned &&
                    (interval.after == null || it.id > interval.after) &&
                    (interval.complete || interval.through?.let { end -> it.id <= end } == true) }.forEach { row ->
                    evicted("Comment", row.id); q.deleteComment(row.id)
                    q.itemAttachments(id).executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) }
                        .filter { it.commentId == row.id }.forEach { evicted("Attachment", it.id); q.deleteAttachment(it.id) }
                }
                putCommentRows(interval.rows)
            }
            if (intervals.comments.isNotEmpty()) {
                val last = pages.last { it.coverage.comments }
                putCommentHistory(id, (mutable.value.commentHistory[id] ?: CommentHistory()).copy(started = true,
                    complete = last.coverage.commentsComplete, nextCursor = last.commentsNextCursor))
            }
            pages.filter { it.coverage.collaboration }.forEach { page ->
                val attachments = requireNotNull(page.attachments)
                val incoming = attachments.mapTo(mutableSetOf()) { it.id }
                q.itemAttachments(id).executeAsList().filter { it.id !in incoming }.forEach { evicted("Attachment", it.id); q.deleteAttachment(it.id) }
                attachments.forEach { row ->
                    requireVisible("Attachment", row.id, id); row.commentId?.let { requireVisible("Comment", it) }
                    q.restoreVisibility("Attachment", row.id); q.putAttachment(row.id, id, null, payload(row))
                }
                val lists = requireNotNull(page.lists)
                // Inherited templates are globally keyed. Only ancestors still in THIS covered lineage grant
                // omission coverage; losing an ancestry link never proves deletion of the former ancestor's list.
                val ancestors = mutableSetOf<String>()
                var parent = item?.splitFromItemId
                while (parent != null && ancestors.add(parent)) parent = mutable.value.items.find { it.id == parent }?.splitFromItemId
                val removed = oldLists.filter { row -> (row.itemId == id || row.mode == ListMode.per_occurrence && row.itemId in ancestors) && lists.none { it.id == row.id } }.distinctBy { it.id }
                touchedLists = touchedLists + removed.map { it.id }
                removed.forEach { evictListRows(it.id) }
                lists.forEach { incoming ->
                    val row = acceptListVisibility(incoming, null)
                    putListRow(row)
                    // Templates carry structure/ACL, not independent occurrence checks. Preserve retained checks.
                    oldLists.filter { it.id == row.id && it.occurrenceId != null && row.mode == ListMode.per_occurrence && !row.cleanupOnly }
                        .forEach { scoped ->
                            val checks = scoped.propertyEntries.associateBy { it.id }
                            putListRow(row.copy(occurrenceId = scoped.occurrenceId, propertyEntries = row.propertyEntries.map { entry ->
                                entry.copy(occurrenceId = scoped.occurrenceId, checkedAt = checks[entry.id]?.checkedAt, checkedBy = checks[entry.id]?.checkedBy)
                            }, gotIt = emptyList()))
                        }
                }
            }
        }
    }
    /** Incremental publication preserves unrelated decoded objects (and does not consult the stream cursor). */
    private fun itemBundleView(id: String, listIds: Set<String>): CacheView {
        val before = mutable.value
        fun <T> replace(old: List<T>, fresh: List<T>, matches: (T) -> Boolean) = old.filterNot(matches) + fresh
        val item = q.getItem(id).executeAsOneOrNull()?.let { WireJson.decodeFromString<ItemRead>(it.payload) }
        val lists = listIds.flatMap { q.listsById(it).executeAsList() }.map { WireJson.decodeFromString<ListResponse>(it.payload) }
        val history = q.itemCommentHistory(id).executeAsOneOrNull()?.let { CommentHistory(it.next_cursor, it.started != 0L, it.complete != 0L, it.older_pages.toInt()) }
        val occurrences = q.itemOccurrences(id).executeAsList().map { WireJson.decodeFromString<OccurrenceRead>(it.payload) }
        val occurrenceIds = occurrences.mapTo(mutableSetOf()) { it.id }
        val boards = q.itemBoards(id).executeAsList().map { WireJson.decodeFromString<ItemRead>(it.payload) }
        val boardIds = boards.mapNotNull { it.occurrence?.id }.toSet()
        val agendas = q.itemAgendas(id).executeAsList().map { WireJson.decodeFromString<ItemRead>(it.payload) }
        val agendaIds = agendas.mapNotNull { it.agenda?.let { a -> a.memberId to a.occurrenceId } }.toSet()
        val reminders = q.itemReminders(id).executeAsList().map { WireJson.decodeFromString<ScheduledReminderRead>(it.payload) }
        val reminderIds = reminders.mapTo(mutableSetOf()) { it.id }
        return before.copy(
            items = replace(before.items, listOfNotNull(item)) { it.id == id }.sortedBy { it.id },
            occurrences = replace(before.occurrences, occurrences) { it.itemId == id || it.id in occurrenceIds }.sortedBy { it.id },
            board = replace(before.board, boards) { it.id == id || it.occurrence?.id in boardIds }
                .sortedWith(compareBy({ it.occurrence?.status }, { it.occurrence?.boardPosition }, { it.occurrence?.id })),
            agenda = replace(before.agenda, agendas) { it.id == id || it.agenda?.let { a -> a.memberId to a.occurrenceId } in agendaIds }.sortedBy { it.agenda?.let { a -> "${a.memberId}:${a.occurrenceId}" } },
            // Split/reparent keeps reminder identities. Drop the former-parent copy before it can drive omission.
            reminders = replace(before.reminders, reminders) { it.itemId == id || it.id in reminderIds }.sortedBy { it.id },
            notifications = replace(before.notifications, q.itemNotifications(id).executeAsList().map { WireJson.decodeFromString<Notification>(it.payload) }) { it.itemId == id }.sortedBy { it.id },
            suggestions = replace(before.suggestions, q.itemPlanning(id).executeAsList().filter { it.kind == "suggestion" }.map { WireJson.decodeFromString<SuggestionResponse>(it.payload) }) { it.itemId == id }.sortedBy { it.id },
            comments = replace(before.comments, q.itemComments(id).executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) }) { it.itemId == id },
            attachments = replace(before.attachments, q.itemAttachments(id).executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) }) { it.itemId == id },
            lists = replace(before.lists, lists) { it.id in listIds },
            itemWrites = (before.itemWrites - id) + q.itemWrites(id).executeAsList().mapNotNull { row ->
                val raw = WireJson.parseToJsonElement(row.payload).jsonObject
                if ("reminder_offsets" in raw) row.id to WireJson.decodeFromJsonElement<ItemResponse>(raw) else null
            }.toMap(),
            evictedItemIds = before.evictedItemIds.let { if (item != null) it - id else it },
            evictedListIds = (before.evictedListIds - lists.filter { it.redacted != true }.map { it.id }.toSet()) +
                (listIds - lists.map { it.id }.toSet()) + lists.filter { it.redacted == true }.map { it.id },
            commentHistory = if (history == null) before.commentHistory - id else before.commentHistory + (id to history),
            sync = mutableSync.value,
        )
    }
    private fun notificationIdsView(ids: Set<String>, rows: List<Notification>): CacheView = mutable.value.let { before ->
        before.copy(notifications = (before.notifications.filterNot { it.id in ids } + rows).sortedBy { it.id }, sync = mutableSync.value)
    }
    private fun commentIdsView(ids: Set<String>, rows: List<Comment>): CacheView = mutable.value.let { before ->
        val removed = ids - rows.mapTo(mutableSetOf()) { it.id }
        before.copy(comments = before.comments.filterNot { it.id in ids } + rows,
            attachments = before.attachments.filterNot { it.commentId in removed }, sync = mutableSync.value)
    }
    private fun commentView(itemId: String): CacheView = mutable.value.let { before ->
        val history = q.itemCommentHistory(itemId).executeAsOneOrNull()?.let { CommentHistory(it.next_cursor, it.started != 0L, it.complete != 0L, it.older_pages.toInt()) }
        before.copy(comments = before.comments.filterNot { it.itemId == itemId } + q.itemComments(itemId).executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) },
            attachments = before.attachments.filterNot { it.itemId == itemId } + q.itemAttachments(itemId).executeAsList().map { WireJson.decodeFromString<Attachment>(it.payload) },
            commentHistory = if (history == null) before.commentHistory - itemId else before.commentHistory + (itemId to history), sync = mutableSync.value)
    }
    private fun listsView(ids: Set<String>): CacheView = mutable.value.let { before ->
        before.copy(lists = before.lists.filterNot { it.id in ids || it.id in changedLists } + (ids + changedLists).flatMap { q.listsById(it).executeAsList() }.map { WireJson.decodeFromString<ListResponse>(it.payload) },
            evictedListIds = q.evictedVisibility("List").executeAsList().toSet(), sync = mutableSync.value)
    }
    private fun membersView(ids: Set<String>, replaceAll: Boolean = false): CacheView = mutable.value.let { before ->
        before.copy(members = ((if (replaceAll) emptyList() else before.members.filterNot { it.id in ids }) + ids.mapNotNull {
            q.getMember(it).executeAsOneOrNull()?.let { row -> WireJson.decodeFromString<MemberRead>(row.payload) }
        }).sortedBy { it.id }, sync = mutableSync.value)
    }

    /** Marks the responses received so far; take it BEFORE requesting a snapshot and pass it to [installStaged]. */
    internal suspend fun receiptMark(): Long = withContext(databaseDispatcher) { mutex.withLock { receiptSeq } }
    /** The receipt mark plus the cursor installed at the same moment, both taken BEFORE requesting a snapshot (N-1). */
    internal data class SnapshotMark(val receipts: Long, val cursor: Long?)
    internal suspend fun snapshotMark(): SnapshotMark =
        withContext(databaseDispatcher) { mutex.withLock { SnapshotMark(receiptSeq, mutableSync.value?.cursor) } }
    /** [receivedAfter]: responses received after this mark may be newer than [snapshot]; inside the same transaction,
     * each one whose version is higher than the snapshot's row replaces it (undated occurrences included, which no
     * windowed refetch can return). [requestCursor]: the cursor installed when [snapshot] was requested; a snapshot
     * older than it is a restore/rollback. Test-only fault injection: [afterStage] runs after every staged row,
     * [beforeCommit] after promotion. */
    internal suspend fun installStaged(fence: Fence, snapshot: SnapshotRead, at: Instant = now(), projections: Projections? = null,
                                       receivedAfter: Long? = null, requestCursor: Long? = null, authoritative: Boolean = false,
                                       scopedChecks: List<ListResponse>? = null, preserveCursor: Boolean = false,
                                       retainedAgendas: List<AgendaWindow> = emptyList(),
                                       beforeCommit: () -> Unit = {}, afterStage: () -> Unit = {}) = transaction(fence, retryCleanup = true, collections = snapshotCollections) {
        // Snapshot receipt repair must not reinstall resources evicted after its request, even on resnapshot.
        fun visible(kind: String, id: String, itemId: String? = null): Boolean = try {
            requireVisible(kind, id, itemId); true
        } catch (_: StaleVisibilityReply) { false }
        val snapshot = snapshot.copy(
            items = snapshot.items.filter { visible("Item", it.id) },
            occurrences = snapshot.occurrences.filter { visible("Occurrence", it.id, it.itemId) && visible("OccurrenceRow", it.id) },
            scheduledReminders = snapshot.scheduledReminders.filter { visible("Occurrence", it.occurrenceId, it.itemId) },
            comments = snapshot.comments.filter { visible("Comment", it.id, it.itemId) },
            notifications = snapshot.notifications.filter { visible("Inbox", "self") && (it.itemId == null || visible("Item", it.itemId)) && (it.occurrenceId == null || visible("Occurrence", it.occurrenceId)) },
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
        val listIds = snapshot.lists.mapTo(mutableSetOf()) { it.id }
        val occurrenceIds = snapshot.occurrences.mapTo(mutableSetOf()) { it.id }
        val commentIds = snapshot.comments.mapTo(mutableSetOf()) { it.id }
        val attachmentIds = snapshot.attachments.mapTo(mutableSetOf()) { it.id }
        mutable.value.lists.map { it.id }.distinct().filter { it !in listIds }.forEach { evicted("List", it) }
        mutable.value.occurrences.filter { it.id !in occurrenceIds }.forEach { evicted("Occurrence", it.id) }
        mutable.value.comments.filter { it.id !in commentIds }.forEach { evicted("Comment", it.id) }
        mutable.value.attachments.filter { it.id !in attachmentIds }.forEach { evicted("Attachment", it.id) }
        val scopedLists = q.allLists().executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.filter { it.occurrenceId != null }
        val ownZone = zoneOf(members, settings.memberId)
        items.forEach { q.restoreVisibility("Item", it.id) }
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
        snapshot.icsSources.forEach { q.putPlanning("feed", it.id, null, payload(it)) }
        snapshot.availability.forEach { q.putPlanning("availability", it.memberId, null, payload(it)) }
        snapshot.suggestions.forEach { q.putPlanning("suggestion", it.id, it.itemId, payload(it)) }
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
        // A targeted fallback is replacement coverage, not permission to skip delivered-but-unapplied frames.
        // Only first sync / terminal reset (or a detected restore) adopts the snapshot's global watermark.
        val cursor = if (preserveCursor && !regressed) mutableSync.value?.cursor ?: snapshot.cursor else snapshot.cursor
        q.putSync(cursor, at.toEpochMilliseconds())
        q.evictOrphanBoards(); q.evictOrphanAgendas()
        projections?.let {
            replaceBoardRows(it.board)
            replaceAgendaRows(it.agenda, snapshot.members, snapshot.memberSettings.memberId)
        }
        retainedAgendas.forEach { window ->
            replaceAgendaRows(window.copy(rows = window.rows.filter { visible("Item", it.id) &&
                (it.agenda == null || visible("Occurrence", it.agenda.occurrenceId)) }), snapshot.members, snapshot.memberSettings.memberId)
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
            if (seq > mark && installed.version < row.version) q.putPlanning("availability", row.memberId, null, payload(row))
        }
        for ((seq, row) in suggestionReceipts.values) {
            val installed = snapshot.suggestions.find { it.id == row.id } ?: continue
            if (seq > mark && installed.version < row.version && snapshot.items.any { it.id == row.itemId })
                q.putPlanning("suggestion", row.id, row.itemId, payload(row))
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
            if (seq > mark && installed.version < row.version) q.putMember(row.id, null, null, payload(row))
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
            if (seq > mark && row.version > installed.version && row.itemId in items) q.putComment(row.id, row.itemId, null, payload(row))
        }
        val attachments = snapshot.attachments.associateBy { it.id }
        for ((seq, row) in attachmentReceipts.values) {
            val installed = attachments[row.id] ?: continue
            if (seq > mark && row.version > installed.version && row.itemId in items) q.putAttachment(row.id, row.itemId, null, payload(row))
        }
        for ((resource, seq) in collaborationDeletions) if (seq > mark) when (resource.first) {
            "list" -> { q.deleteList(resource.second); q.evictEntry(resource.second) }
            "comment" -> { q.deleteComment(resource.second); evictCommentAttachments(resource.second) }
            "attachment" -> q.deleteAttachment(resource.second)
        }
        settingsReceipt?.let { (seq, row) ->
            val installed = snapshot.memberSettings
            if (seq > mark && installed.memberId == row.memberId && installed.version < row.version) {
                q.clearSettingss(); q.putSettings(row.memberId, null, null, payload(row))
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
        val expiredOccurrences = q.allOccurrences().executeAsList().filter { it.expires_at?.let { expiry -> expiry < cutoff } == true }.mapTo(mutableSetOf()) { it.id }
        q.cleanOccurrences(cutoff); q.cleanAgendas(cutoff); q.cleanReminders(cutoff)
        occurrenceObservations.keys.removeAll(expiredOccurrences)
        scopedListRows().filter { it.occurrence_id in expiredOccurrences }.forEach { evictListScopeRows(it.id, it.occurrence_id) }
        trimListScopes()
    }
    /** The board projection is one scope, fully paginated: replaced as a whole inside the caller's transaction. */
    private fun replaceBoardRows(rows: List<ItemRead>) {
        if (q.allProjectionScopes().executeAsList().none { it.kind == "board" }) q.putProjectionScope("board", "board", "household")
        q.clearBoards()
        rows.forEach { row -> requireVisible("Item", row.id); q.restoreVisibility("Item", row.id); row.occurrence?.let { requireVisible("Occurrence", it.id); q.putBoard(it.id, row.id, null, payload(row)) } }
    }
    /** Replaces only the rows the fetched window provably covers for its member; rows of other members and outside the
     * window stay until their own refetch, eviction or cleanup. */
    private fun replaceAgendaRows(window: AgendaWindow, members: List<MemberRead>, ownMember: String?) {
        val member = window.memberId ?: ownMember
        if (member != null) {
            val overlapping = q.allProjectionScopes().executeAsList().filter { it.kind == "agenda" }.map { it to WireJson.decodeFromString<ItemAgendaWindow>(it.payload) }
                .filter { (_, scope) -> scope.memberId == member && scope.from <= window.to && scope.to >= window.from }
            val scope = ItemAgendaWindow(member, minOf(window.from, overlapping.minOfOrNull { it.second.from } ?: window.from),
                maxOf(window.to, overlapping.maxOfOrNull { it.second.to } ?: window.to))
            overlapping.forEach { q.removeProjectionScope("agenda", it.first.id) }
            val payload = payload(scope)
            q.putProjectionScope("agenda", payload, payload)
        }
        for (row in q.allAgendas().executeAsList()) {
            if (member != null && row.id.substringBefore(':') != member) continue
            val agenda = WireJson.decodeFromString<ItemRead>(row.payload).agenda ?: continue
            if (provablyInAgendaWindow(agenda, window.from, window.to, zoneOf(members, agenda.memberId))) q.deleteAgenda(row.id)
        }
        window.rows.forEach { row ->
            requireVisible("Item", row.id); q.restoreVisibility("Item", row.id)
            row.agenda?.let { requireVisible("Occurrence", it.occurrenceId); q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, members), payload(row), it.occurrenceId) }
        }
    }
    suspend fun cleanup(at: Instant = now()) = transaction { cleanRows(at) }
    suspend fun clear() = transaction(fenced = false) { clearRows(); q.clearCacheOwner() }
    suspend fun retryAttachmentCleanup() = transaction(retryCleanup = true) {}
    /** Explicit storage repair retries bypass only the clock, never the durable purge obligation. */
    internal suspend fun allowCleanupRetryNow() = withContext(databaseDispatcher) { mutex.withLock {
        if (closed) throw CancellationException("Read cache closed")
        db.transaction {
            q.getCleanupFailure().executeAsOneOrNull()?.let { q.putCleanupFailure(it.attempts, now().toEpochMilliseconds()) }
        }
    } }
    /** Explicit recovery discards only disposable household data, never trust or delivery metadata.
     * The durable clear obligation survives a failed wipe; no refresh/ack can bypass it. */
    suspend fun clearLocalCache() = transaction(fenced = false, localReset = true, retryCleanup = true) {
        clearRows(); q.clearAttachmentCleanup(); q.putAttachmentCleanup("clear", "recovery"); q.clearCleanupFailure()
    }
    private fun clearRows() {
        occurrenceObservations.clear()
        clearEntities(); q.clearPlanning(); q.clearProjectionScopes(); q.clearAgendas(); q.clearBoards(); q.clearSync(); q.clearPendingSync(); q.clearStaging(); q.clearVisibility(); q.clearCommentHistories()
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
    internal suspend fun hideInbox(fence: Fence) = transaction(fence, collections = setOf("inbox"), publication = { mutable.value.copy(notifications = emptyList(), sync = mutableSync.value) }) { evicted("Inbox", "self"); q.clearNotifications() }
    suspend fun replaceNotifications(fence: Fence, rows: List<Notification>) = transaction(fence, collections = setOf("inbox"), publication = { mutable.value.copy(notifications = rows.sortedBy { it.id }, sync = mutableSync.value) }) {
        requireVisible("Inbox", "self")
        rows.forEach { row ->
            row.itemId?.let { requireVisible("Item", it) }
            row.occurrenceId?.let { requireVisible("Occurrence", it) }
        }
        q.clearNotifications(); rows.forEach { q.putNotification(it.id, it.itemId, null, payload(it)) }
    }
    /** Exact requested-id absence is omission coverage only for those IDs. */
    internal suspend fun replaceNotificationIds(fence: Fence, ids: Set<String>, rows: List<Notification>) = transaction(fence, collections = setOf("inbox"), publication = { notificationIdsView(ids, rows) }) {
        requireVisible("Inbox", "self")
        require(rows.all { it.id in ids })
        rows.forEach { row ->
            row.itemId?.let { requireVisible("Item", it) }
            row.occurrenceId?.let { requireVisible("Occurrence", it) }
        }
        ids.forEach { q.deleteNotification(it) }
        rows.forEach { q.putNotification(it.id, it.itemId, null, payload(it)) }
    }
    internal suspend fun replaceCommentIds(fence: Fence, itemId: String, ids: Set<String>, rows: List<Comment>) = transaction(fence, collections = setOf("comments"), publication = { commentIdsView(ids, rows) }, retryCleanup = true) {
        requireVisible("Item", itemId)
        require(rows.all { it.id in ids && it.itemId == itemId })
        val returned = rows.mapTo(mutableSetOf()) { it.id }
        (ids - returned).forEach { id ->
            evicted("Comment", id); q.deleteComment(id); evictCommentAttachments(id, itemId)
            collaborationDeletions["comment" to id] = ++receiptSeq
        }
        putCommentRows(rows)
    }
    suspend fun putNotificationPrefs(fence: Fence, row: PrefsRead) = transaction(fence, provisional = true, collections = setOf("prefs"), publication = { mutable.value.copy(notificationPrefs = row, sync = mutableSync.value) }) { q.putNotificationPrefs(row.memberId, null, null, payload(row)) }
    suspend fun acknowledgeCursor(cursor: Long) = transaction(cursorOnly = true) { require(cursor >= 0); q.advanceCursor(cursor) }
    /** A 204 occurrence mutation can regenerate away this identity; remove all of its projections. */
    suspend fun evictOccurrence(id: String) = transaction { evictOccurrenceRows(id) }
    private fun requireOccurrenceRowVisible(id: String, itemId: String) {
        requireVisible("Occurrence", id, itemId); requireVisible("OccurrenceRow", id)
    }
    private fun removeScopedOccurrence(id: String) {
        // Fence only old occurrence-row replies, never independently covered projections/children.
        evicted("OccurrenceRow", id); q.restoreVisibility("OccurrenceRow", id)
        q.deleteOccurrence(id); occurrenceReceipts.remove(id); occurrenceObservations.remove(id)
    }
    private fun evictOccurrenceRows(id: String) {
        evicted("Occurrence", id)
        q.deleteOccurrence(id)
        occurrenceReceipts.remove(id)
        mutable.value.notifications.filter { it.occurrenceId == id }.forEach { q.deleteNotification(it.id) }
        mutable.value.suggestions.filter { it.occurrenceId == id }.forEach { q.removePlanning("suggestion", it.id) }
        scopedListRows().filter { it.occurrence_id == id }.forEach { evictListScopeRows(it.id, id) }
        q.evictOccurrenceBoard(id); q.evictOccurrenceAgenda(id); q.evictOccurrenceReminders(id)
    }
    internal suspend fun replaceSources(fence: Fence, rows: List<SourceSnapshot>) = transaction(fence, collections = setOf("feeds")) {
        q.allPlanning().executeAsList().filter { it.kind == "feed" }.forEach { q.removePlanning("feed", it.id) }
        rows.forEach { q.putPlanning("feed", it.id, null, payload(it)) }
    }
    suspend fun putAvailability(fence: Fence, row: AvailabilityResponse) = transaction(fence, provisional = true, collections = setOf("planning"), publication = {
        mutable.value.copy(availability = mutable.value.availability.filterNot { it.memberId == row.memberId } + listOfNotNull(
            q.getPlanning("availability", row.memberId).executeAsOneOrNull()?.let { WireJson.decodeFromString<AvailabilityResponse>(it.payload) }), sync = mutableSync.value)
    }) {
        availabilityReceipts.receive(row.memberId, row) { it.version }
        val old = mutable.value.availability.find { it.memberId == row.memberId }
        if (old == null || row.version >= old.version) q.putPlanning("availability", row.memberId, null, payload(row))
    }
    private fun requireSuggestionVisible(row: SuggestionResponse) {
        requireVisible("Item", row.itemId); requireVisible("Occurrence", row.occurrenceId)
    }
    suspend fun putSuggestion(fence: Fence, row: SuggestionResponse) = transaction(fence, provisional = true, collections = setOf("planning")) {
        requireSuggestionVisible(row)
        suggestionReceipts.receive(row.id, row) { it.version }
        val old = mutable.value.suggestions.find { it.id == row.id }
        if (old == null || row.version >= old.version) q.putPlanning("suggestion", row.id, row.itemId, payload(row))
    }
    suspend fun replaceSuggestions(fence: Fence, member: String?, rows: List<SuggestionResponse>) = transaction(fence, collections = setOf("planning")) {
        rows.forEach { requireSuggestionVisible(it) }
        mutable.value.suggestions.filter { member == null || it.memberId == member }.forEach { q.removePlanning("suggestion", it.id) }
        rows.forEach { q.putPlanning("suggestion", it.id, it.itemId, payload(it)) }
    }
    suspend fun evictItem(id: String) = withContext(NonCancellable) {
        transaction(retryCleanup = true) { evictItemRows(id); visibilityDeletions[id] = ++receiptSeq }
    }
    private fun evictItemRows(id: String) {
        evicted("Item", id)
        // Inherited list scopes are keyed by the ancestor list, not this item's id.
        val occurrences = q.itemOccurrences(id).executeAsList().map { it.id } +
            q.itemBoards(id).executeAsList().mapNotNull { WireJson.decodeFromString<ItemRead>(it.payload).occurrence?.id } +
            q.itemAgendas(id).executeAsList().mapNotNull { WireJson.decodeFromString<ItemRead>(it.payload).agenda?.occurrenceId }
        occurrences.toSet().forEach { evictOccurrenceRows(it) }
        q.evictNotifications(id)
        q.evictPlanning(id)
        q.evictEntriesByItem(id)
        q.evictList(id); q.evictComment(id); q.evictAttachment(id); q.deleteCommentHistory(id)
        q.evictItem(id); q.evictOccurrence(id); q.evictAgenda(id); q.evictBoard(id); q.evictReminder(id); q.evictWrite(id)
    }
    suspend fun putList(fence: Fence, row: ListResponse, requestedAfter: Long? = null): ListResponse {
        var accepted: ListResponse? = null
        try {
            transaction(fence, provisional = true, collections = setOf("lists"), publication = { listsView(setOf(row.id)) }) { accepted = receiveList(row, requestedAfter) }
        } catch (stale: StaleCollectionReply) {
            // Retain the pre-existing redacted-descriptor contract without retrying an old private detail read.
            transaction(fence, publication = { mutable.value.copy(sync = mutableSync.value) }) {
                accepted = redactedList(row.id) ?: throw stale
            }
        }
        return accepted ?: throw CancellationException("List visibility changed")
    }
    suspend fun replaceLists(fence: Fence, itemId: String?, rows: List<ListResponse>, requestedAfter: Long? = null): List<ListResponse> {
        var accepted = emptyList<ListResponse>()
        try { transaction(fence, collections = setOf("lists"), publication = { listsView(emptySet()) }) {
            val mark = requestedAfter ?: (fence as Lease).visibility
            val newerLists = listReceipts.values.filter { it.first > mark }.map { it.second }
            val newerEntries = entryReceipts.values.filter { it.first > mark }.map { it.second }
            val retained = newerLists.mapTo(mutableSetOf()) { it.id } + newerEntries.map { it.listId }
            mutable.value.lists.filter { (itemId == null || it.itemId == itemId) && it.id !in retained && rows.none { row -> row.id == it.id } }
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
        } } catch (stale: StaleCollectionReply) {
            if (currentCoroutineContext()[TargetedReadPolicy] != null || mutable.value.visibilityRevision <= (fence as Lease).visibility) throw stale
            // An old UI collection may return the already-fenced cache, but grants no replacement coverage.
            transaction(fence, publication = { mutable.value.copy(sync = mutableSync.value) }) {
                accepted = mutable.value.lists.filter { it.occurrenceId == null && (itemId == null || it.itemId == itemId) }
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
        if (!row.cleanupOnly && row.itemId?.let { referenceHidden("Item", it) } == true) throw StaleVisibilityReply()
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
        changedLists += id
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
    suspend fun putEntry(fence: Fence, row: EntryResponse) = transaction(fence, provisional = true, collections = setOf("lists"), publication = { listsView(setOf(row.listId)) }) {
        if (redactedList(row.listId) == null) { putEntryRow(row); entryReceipts.receive("${row.id}:${row.occurrenceId.orEmpty()}", row) { it.version } }
    }
    private fun putEntryRow(row: EntryResponse) {
        requireVisible("List", row.listId); requireVisible("Entry", row.id)
        if (redactedList(row.listId) != null) return
        val list = q.listsById(row.listId).executeAsList().map { WireJson.decodeFromString<ListResponse>(it.payload) }.find { it.occurrenceId == row.occurrenceId } ?: return
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
            q.putList(row.id, row.occurrenceId.orEmpty(), row.itemId, null, payload(retained))
        }
    }
    private fun putListRow(row: ListResponse) {
        changedLists += row.id
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
        q.putList(row.id, row.occurrenceId.orEmpty(), row.itemId, null, payload(row))
        q.evictEntryScope(row.id, row.occurrenceId.orEmpty())
        (row.propertyEntries + row.gotIt).forEach { q.restoreVisibility("Entry", it.id); q.putEntry(it.id, row.occurrenceId.orEmpty(), row.id, null, payload(it)) }
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
    private fun scopedListRows() = q.listScopes().executeAsList()
    /** Retain at most 32 scopes (or the active editors, if more). Templates do not count.
     * ORDER BY rowid / INSERT OR REPLACE keeps the most recently fetched inactive scopes. */
    private fun trimListScopes() {
        val scopes = scopedListRows()
        scopes.filterNot { (it.id to it.occurrence_id) in activeListScopes }
            .take((scopes.size - 32).coerceAtLeast(0)).forEach { evictListScopeRows(it.id, it.occurrence_id) }
    }
    private fun evictListScopeRows(id: String, occurrenceId: String) {
        changedLists += id
        q.deleteListScope(id, occurrenceId); q.evictEntryScope(id, occurrenceId)
        listReceipts.remove("$id:$occurrenceId")
        val keys = entryReceipts.filterValues { it.second.listId == id && it.second.occurrenceId == occurrenceId }.keys.toList()
        keys.forEach { entryReceipts.remove(it) }
    }
    suspend fun evictListScope(id: String, occurrenceId: String) = transaction(collections = setOf("lists"), publication = { listsView(setOf(id)) }) { evicted("ListScope", "$id:$occurrenceId"); evictListScopeRows(id, occurrenceId) }
    suspend fun evictList(id: String) = transaction(collections = setOf("lists"), publication = { listsView(setOf(id)) }) { evictListRows(id) }
    suspend fun evictComment(id: String) = transaction(collections = setOf("comments"), publication = { commentIdsView(setOf(id), emptyList()) }, retryCleanup = true) {
        evicted("Comment", id); q.deleteComment(id)
        val parent = mutable.value.comments.find { it.id == id }?.itemId ?: mutable.value.attachments.find { it.commentId == id }?.itemId
        parent?.let { evictCommentAttachments(id, it) }
        collaborationDeletions["comment" to id] = ++receiptSeq
    }
    private fun evictCommentAttachments(id: String, itemId: String? = null) {
        val rows = if (itemId == null) q.allAttachments().executeAsList() else q.itemAttachments(itemId).executeAsList()
        rows.map { WireJson.decodeFromString<Attachment>(it.payload) }.filter { it.commentId == id }.forEach { q.deleteAttachment(it.id) }
    }
    suspend fun putComments(fence: Fence, rows: List<Comment>) = transaction(fence, provisional = true, collections = setOf("comments"), publication = { commentIdsView(rows.mapTo(mutableSetOf()) { it.id }, rows) }) { putCommentRows(rows) }
    private fun putCommentRows(rows: List<Comment>) {
        rows.forEach { requireVisible("Comment", it.id, it.itemId); q.restoreVisibility("Comment", it.id); q.putComment(it.id, it.itemId, null, payload(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    private fun putCommentHistory(itemId: String, history: CommentHistory) = q.putCommentHistory(itemId, history.nextCursor,
        if (history.started) 1 else 0, if (history.complete) 1 else 0, history.olderPages.toLong())
    suspend fun setCommentWindow(fence: Fence, itemId: String, olderPages: Int) = transaction(fence) {
        requireVisible("Item", itemId)
        putCommentHistory(itemId, (mutable.value.commentHistory[itemId] ?: CommentHistory()).copy(olderPages = olderPages.coerceAtLeast(0)))
    }
    suspend fun putCommentPage(fence: Fence, itemId: String, cursor: String?, page: CommentPage) = transaction(fence, collections = setOf("comments"), publication = { commentView(itemId) }, retryCleanup = true) {
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
    suspend fun replaceComments(fence: Fence, itemId: String, rows: List<Comment>) = transaction(fence, collections = setOf("comments"), publication = { commentView(itemId) }, retryCleanup = true) {
        replaceCommentRows(fence, itemId, rows)
        putCommentHistory(itemId, (mutable.value.commentHistory[itemId] ?: CommentHistory()).copy(started = true, complete = true, nextCursor = null))
    }
    private fun replaceCommentRows(fence: Fence, itemId: String, rows: List<Comment>) {
        requireVisible("Item", itemId)
        val newer = commentReceipts.values.filter { it.first > (fence as Lease).visibility && it.second.itemId == itemId }.map { it.second }
        val accepted = rows.associateByTo(mutableMapOf()) { it.id }
        newer.forEach { row -> if ((accepted[row.id]?.version ?: 0) <= row.version) accepted[row.id] = row }
        q.itemComments(itemId).executeAsList().map { WireJson.decodeFromString<Comment>(it.payload) }.filter { it.id !in accepted }.forEach {
            evicted("Comment", it.id); collaborationDeletions["comment" to it.id] = ++receiptSeq; evictCommentAttachments(it.id, itemId)
        }
        q.evictComment(itemId); accepted.values.forEach { requireVisible("Comment", it.id, it.itemId); q.restoreVisibility("Comment", it.id); q.putComment(it.id, it.itemId, null, payload(it)); commentReceipts.receive(it.id, it) { row -> row.version }; collaborationDeletions.remove("comment" to it.id) }
    }
    suspend fun putAttachment(fence: Fence, row: Attachment) = transaction(fence, retryCleanup = true) { requireVisible("Attachment", row.id, row.itemId); row.commentId?.let { requireVisible("Comment", it) }; q.restoreVisibility("Attachment", row.id); q.putAttachment(row.id, row.itemId, null, payload(row)); attachmentReceipts.receive(row.id, row) { it.version }; collaborationDeletions.remove("attachment" to row.id) }
    suspend fun evictAttachment(id: String) = withContext(NonCancellable) {
        transaction(retryCleanup = true) { evicted("Attachment", id); q.deleteAttachment(id); collaborationDeletions["attachment" to id] = ++receiptSeq }
    }
    /** Serialize private viewer creation with metadata eviction, even without a trust controller in tests. */
    internal suspend fun withLiveAttachment(row: Attachment, action: suspend () -> Unit) = withContext(databaseDispatcher) {
        mutex.withLock {
            val fence = sessionCommitMutex
            fence?.lock()
            try {
                currentCoroutineContext().ensureActive()
                val lease = currentCoroutineContext()[Lease]
                requireVisible("Attachment", row.id, row.itemId, lease?.visibility)
                row.commentId?.let { requireVisible("Comment", it, receivedAt = lease?.visibility) }
                if (lease != null && (lease.epoch != sessionEpoch() || lease.storage != storageGeneration || lease.revision != lease.currentRevision()))
                    throw CancellationException("Household session or cache changed")
                require(mutable.value.attachments.any { it.id == row.id && it.version == row.version && it.deletedAt == null }) { "Attachment is no longer visible" }
                drainAttachmentCleanup()
                action()
            } finally { fence?.unlock() }
        }
    }
    suspend fun replaceMembers(fence: Fence, rows: List<MemberRead>, receivedAfter: Long? = null) = transaction(fence, collections = setOf("members"), publication = { membersView(rows.mapTo(mutableSetOf()) { it.id }, replaceAll = true) }) {
        val incoming = rows.associateBy { it.id }
        q.clearMembers(); rows.forEach { q.putMember(it.id, null, null, payload(it)) }
        receivedAfter?.let { mark ->
            for ((seq, row) in memberReceipts.values) {
                val installed = incoming[row.id] ?: continue
                if (seq > mark && row.version > installed.version) q.putMember(row.id, null, null, payload(row))
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
    suspend fun putMember(fence: Fence, row: MemberRead) = transaction(fence, provisional = true, collections = setOf("members"), publication = { membersView(setOf(row.id)) }) {
        val installed = q.getMember(row.id).executeAsOneOrNull()?.let { WireJson.decodeFromString<MemberRead>(it.payload) }
        if (installed == null || installed.version <= row.version) q.putMember(row.id, null, null, payload(row))
        memberReceipts.receive(row.id, row) { it.version }
    }
    suspend fun putSettings(fence: Fence, row: SettingsRead) = transaction(fence, provisional = true, collections = setOf("settings"), publication = { mutable.value.copy(settings = q.getSettings(row.memberId).executeAsOneOrNull()?.let { WireJson.decodeFromString<SettingsRead>(it.payload) }, sync = mutableSync.value) }) {
        val installed = q.getSettings(row.memberId).executeAsOneOrNull()?.let { WireJson.decodeFromString<SettingsRead>(it.payload) }
        if (installed == null || installed.memberId != row.memberId || installed.version <= row.version) {
            q.clearSettingss(); q.putSettings(row.memberId, null, null, payload(row))
        }
        val previous = settingsReceipt?.second
        settingsReceipt = ++receiptSeq to
            (if (previous != null && previous.memberId == row.memberId && previous.version > row.version) previous else row)
    }
    suspend fun replaceBoard(fence: Fence, rows: List<ItemRead>, scope: String? = null) = transaction(fence, collections = setOf("board")) {
        require(scope == null || scope in setOf("mine", "household"))
        replaceBoardRows(rows)
        if (scope != null) q.putProjectionScope("board", "board", scope)
    }
    /** Agenda refetch: replaces in place only what [window] authoritatively covered. */
    suspend fun replaceAgenda(fence: Fence, window: AgendaWindow) = transaction(fence, collections = setOf("agenda")) {
        mutable.value.let { replaceAgendaRows(window, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** Replaces every cached agenda row (test seeding; production refetches use [AgendaWindow]). */
    internal suspend fun replaceAgenda(fence: Fence, rows: List<ItemRead>) = transaction(fence, collections = setOf("agenda")) {
        q.clearAgendas()
        rows.forEach { row -> row.agenda?.let { q.putAgenda(it.memberId + ":" + it.occurrenceId, row.id, agendaExpiry(row, mutable.value.members), payload(row), it.occurrenceId) } }
        cleanRows(now())
    }
    /** Write follow-up (R59): board and agenda switch together, in place, never cleared ahead of a fetch. */
    suspend fun replaceProjections(fence: Fence, projections: Projections) = transaction(fence, collections = setOf("board", "agenda")) {
        replaceBoardRows(projections.board)
        mutable.value.let { replaceAgendaRows(projections.agenda, it.members, it.settings?.memberId) }
        cleanRows(now())
    }
    /** A windowed `/v1/items/{id}/occurrences` answer never contains undated occurrences, nor anything outside
     * [from, to): only returned rows and cached rows the window provably covered (now gone) are replaced (I-2). The
     * window is required: there is no "replace everything" default that could drop undated rows (N-3). */
    suspend fun putOccurrences(fence: Fence, itemId: String, rows: List<OccurrenceResponse>, from: Instant, to: Instant,
                               includeUndated: Boolean = false, receivedAfter: Long? = null) = transaction(fence, collections = setOf("occurrences")) {
        val returned = rows.mapTo(HashSet()) { it.id }
        // m-7: whether dated-window or undated coverage, a missing row is deleted only if nothing newer than the request
        // (an own-write response or a snapshot install) observed it.
        mutable.value.occurrences.filter { it.itemId == itemId && it.id !in returned &&
            (provablyInWindow(it, from, to) || (includeUndated && it.startsAt == null && it.dueAt == null && it.localStart == null && it.localDue == null && it.dueDate == null)) &&
            (receivedAfter?.let { mark -> (occurrenceObservations[it.id] ?: 0L) <= mark } ?: (it.id !in occurrenceReceipts)) }
            .forEach { removeScopedOccurrence(it.id) }
        val items = mutable.value.items.associateBy { it.id }
        val occurrences = mutable.value.occurrences.associateBy { it.id }
        rows.forEach { putOccurrenceRow(it, items, occurrences) }
        cleanRows(now())
    }
    suspend fun putItem(fence: Fence, response: ItemResponse) = putItems(fence, listOf(response))
    /** One transaction for a whole page of item reads/writes. */
    suspend fun putItems(fence: Fence, responses: List<ItemResponse>) = transaction(fence, provisional = true, collections = setOf("items")) {
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
        q.putItem(row.id, row.id, null, payload(row))
        visibilityDeletions.remove(row.id)
        // itemWrites: the latest item-shaped response (write or GET) with its reminder offsets, until the next snapshot.
        q.putWrite(response.id, response.id, null, payload(response))
    }
    /** A write's authoritative response for one occurrence (also kept as a receipt, see [installStaged]). */
    suspend fun putOccurrence(fence: Fence, response: OccurrenceResponse) = transaction(fence, provisional = true, collections = setOf("occurrences")) {
        putOccurrenceRow(response, mutable.value.items.associateBy { it.id }, mutable.value.occurrences.associateBy { it.id })
        occurrenceReceipts.receive(response.id, response) { it.version }
    }
    private fun putOccurrenceRow(response: OccurrenceResponse, items: Map<String, ItemRead>, occurrences: Map<String, OccurrenceRead>,
                                 zone: TimeZone = ownZone()) {
        requireOccurrenceRowVisible(response.id, response.itemId); q.restoreVisibility("Occurrence", response.id)
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
        q.putOccurrence(row.id, row.itemId, expiry(row, zone, items[row.itemId]), payload(row))
        occurrenceObservations[row.id] = ++receiptSeq
        // Snapshot receipt reapplication may pass an older item map; only update the installed parent's flag.
        q.getItem(row.itemId).executeAsOneOrNull()?.let { stored ->
            val item = WireJson.decodeFromString<ItemRead>(stored.payload)
            q.putItem(item.id, item.id, null, payload(item.copy(pendingDerivation = response.pendingDerivation)))
        }
    }
    fun close() = runBlocking(databaseDispatcher) { mutex.withLock {
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
