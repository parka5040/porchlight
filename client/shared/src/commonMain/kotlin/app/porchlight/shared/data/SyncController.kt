package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.*
import app.porchlight.shared.transport.ApiProblem
import app.porchlight.shared.trust.TrustController
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlin.coroutines.ContinuationInterceptor
import kotlin.coroutines.CoroutineContext
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.seconds
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Instant

sealed interface SyncStatus {
    data object Idle : SyncStatus
    data object Refreshing : SyncStatus
    data object Ready : SyncStatus
    data class Failed(val code: String) : SyncStatus
}

class SyncController(private val api: ReadsApi, private val cache: ReadCache,
                     private val items: ItemRepository, private val occurrences: OccurrenceRepository,
                     private val household: HouseholdRepository, private val agenda: AgendaRepository,
                     private val board: BoardRepository, private val now: () -> Instant,
                     private val version: String = BuildInfo.VERSION, private val call: GeneratedCall? = null,
                     dispatcher: CoroutineDispatcher = Dispatchers.Default) {
    private val mutex = Mutex()
    private val refreshScope = CoroutineScope(SupervisorJob() + dispatcher)
    private val targetedSlots = Semaphore(2)
    // Shipping stays gated until (c)'s differential proof; capability alone is not the equivalence gate.
    internal var targetingEnabled: () -> Boolean = { false }
    internal var targetedRead: suspend (ReadCache.Fence, SyncDemand) -> Boolean = { _, _ -> true }
    internal var targetedOrder: suspend (SyncDemand, suspend () -> Boolean) -> Boolean = { _, operation -> operation() }
    internal var targetedFollowUp: suspend (SyncDemand) -> Unit = {}
    internal var targetedDemandAdmitted: () -> Unit = {}
    internal var waitForTargetedBatch: suspend (Long) -> Unit = { delay(it) }
    private val targeted = TargetedSyncScheduler(cache, call, refreshScope, targetedSlots,
        { fence, demand -> targetedRead(fence, demand) }, { refresh(authoritative = true) }, now).also {
        it.admitted = { targetedDemandAdmitted() }
        it.waitForBatch = { wait -> waitForTargetedBatch(wait) }
        it.orderRead = { demand, operation -> targetedOrder(demand, operation) }
        it.afterRead = { targetedFollowUp(it) }
    }
    internal suspend fun reconcile(vararg demands: SyncDemand) = targeted.request(demands.toList())
    internal suspend fun targetedSessionChanged() = targeted.sessionChanged()
    private var snapshotIdentity: Triple<Long, Long, Long>? = null
    private var snapshotNotBefore: Instant? = null
    private suspend fun snapshotStart(fence: ReadCache.Fence) {
        if (!targetingEnabled()) return
        val lease = cache.refreshKey(fence)
        val revision = call?.withReadSession { it } ?: 0L
        val identity = Triple(lease.first, lease.second, revision)
        if (snapshotIdentity != identity) { snapshotIdentity = identity; snapshotNotBefore = null }
        snapshotNotBefore?.let { waitForReadRetry((it - now()).inWholeMilliseconds.coerceAtLeast(0)) }
        // Waits own no refresh mutex or read permits, and confer no replacement-session admission.
        cache.refreshKey(fence)
        call?.withReadSession { }
    }
    private val demandLock = Mutex()
    private val projectionSlots = Semaphore(3)
    internal var waitForReadRetry: suspend (Long) -> Unit = { delay(it) }
    internal var refreshDemandAdmitted: () -> Unit = {}
    private sealed interface RefreshNeed {
        data object Snapshot : RefreshNeed
        data object AuthoritativeSnapshot : RefreshNeed
        data object ResetCursor : RefreshNeed
    }
    private class Cohort(val context: CoroutineContext, val admission: ReadCache.Fence) {
        val needs = mutableSetOf<RefreshNeed>()
        val result = CompletableDeferred<Unit>()
        var visibility: Long? = null // null until this cohort's read actually starts
    }
    private class RefreshFlight(val key: Triple<Long, Long, Long>, first: Cohort) {
        var active = first
        var pending: Cohort? = null
        var job: Job? = null
    }
    private var flight: RefreshFlight? = null
    private val admittedFlights = MutableStateFlow<Set<RefreshFlight>>(emptySet())
    internal suspend fun refreshSessionChanged(epoch: Long, storage: Long, revision: Long) {
        demandLock.withLock {
            admittedFlights.value.filter { it.key != Triple(epoch, storage, revision) }.forEach { it.job?.cancel() }
        }
    }
    /** Application-owned: cancelling a waiter never cancels a cohort's work or other waiters. */
    suspend fun refresh(authoritative: Boolean = false) = requestRefresh(authoritative, null)
    internal suspend fun refreshForEvent(cursor: Long) = requestRefresh(false, cursor)
    internal suspend fun resnapshot() = requestRefresh(true, null, resetCursor = true)
    private suspend fun requestRefresh(authoritative: Boolean, eventCursor: Long?, resetCursor: Boolean = false) {
        if (call == null) admitRefresh(authoritative, eventCursor, 0L, resetCursor)
        else call.withReadSession { revision -> admitRefresh(authoritative, eventCursor, revision, resetCursor) }
    }
    private suspend fun admitRefresh(authoritative: Boolean, eventCursor: Long?, revision: Long, resetCursor: Boolean = false): Unit = cache.inSession { fence ->
        if (eventCursor != null && eventCursor <= (cache.sync.value?.cursor ?: -1)) return@inSession
        val lease = cache.refreshKey(fence)
        val key = Triple(lease.first, lease.second, revision)
        val context = currentCoroutineContext().minusKey(Job).minusKey(ContinuationInterceptor)
        val (cohort, observing) = demandLock.withLock {
            refreshScope.coroutineContext.ensureActive()
            val current = flight?.takeIf { it.key == key }
            val activeMark = current?.active?.visibility
            if (eventCursor != null) refreshDemandAdmitted()
            // An echo observes an already-started read before asking for work. A later visibility fence cannot
            // observe that older read, even when its cursor covers the event. Own-write receipts aren't evictions.
            if (eventCursor != null && current != null && activeMark != null &&
                !(lease.third > activeMark && cache.view.value.visibilityRevision > activeMark)) {
                current.active to true
            } else {
                val next = if (current == null) {
                    val obsolete = flight
                    obsolete?.job?.cancel()
                    val first = Cohort(context, fence)
                    val created = RefreshFlight(key, first)
                    flight = created
                    admittedFlights.update { it + created }
                    created.job = refreshScope.launch {
                        obsolete?.job?.join()
                        drain(created)
                    }
                    created.job!!.invokeOnCompletion { failure ->
                        // Also covers close racing the launch before drain's finally block can be entered.
                        if (failure != null) first.result.completeExceptionally(failure)
                        admittedFlights.update { it - created }
                    }
                    first
                } else if (activeMark == null) current.active
                else current.pending ?: Cohort(context, fence).also { current.pending = it }
                next.needs += if (authoritative) RefreshNeed.AuthoritativeSnapshot else RefreshNeed.Snapshot
                if (resetCursor) next.needs += RefreshNeed.ResetCursor
                next to false
            }
        }
        cohort.result.await()
        // Only a cursor-covered echo is satisfied by observing a read started before the event was received.
        if (observing && eventCursor != null && eventCursor > (cache.sync.value?.cursor ?: -1))
            admitRefresh(false, eventCursor, revision)
    }
    private suspend fun drain(current: RefreshFlight) {
        try {
            while (true) {
                val cohort = current.active
                var failure: Exception? = null
                try {
                    withContext(cohort.context) {
                        refreshOnce(cohort) {
                            demandLock.withLock { if (flight === current && current.pending == null) flight = null }
                        }
                    }
                } catch (e: Exception) {
                    failure = e
                    currentCoroutineContext().ensureActive()
                }
                val again = demandLock.withLock {
                    val pending = current.pending
                    if (pending == null) { if (flight === current) flight = null; false }
                    else { current.active = pending; current.pending = null; true }
                }
                // Retire/advance before waking observers: an uncovered echo can then observe the next cohort
                // without looping on this completed read or demanding redundant work behind a newer read.
                if (failure == null) cohort.result.complete(Unit) else cohort.result.completeExceptionally(failure)
                if (!again) return
            }
        } finally {
            withContext(NonCancellable) {
                demandLock.withLock {
                    if (flight === current) flight = null
                    val cancelled = CancellationException("Household refresh stopped")
                    current.active.result.completeExceptionally(cancelled)
                    current.pending?.result?.completeExceptionally(cancelled)
                }
            }
        }
    }
    /** Startup opens the stream from a cursor no already-admitted snapshot can subsequently replace.
     * Capture a finite barrier: ordinary refresh/event waiters still await only their own cohort. */
    internal suspend fun awaitAdmittedRefreshes() {
        val admitted = demandLock.withLock { admittedFlights.value.flatMap { listOfNotNull(it.active.result, it.pending?.result) } }
        admitted.forEach { it.join() }
    }
    internal fun close() { refreshScope.cancel() }
    // N-1: items refetched (after an occurrence write or a C4 tickle) while a snapshot was in flight. That snapshot may
    // predate them, so after installing it they are refetched once more. Guarded by overlapLock.
    private val overlapLock = Mutex()
    private var snapshotInFlight = false
    private val overlapped = mutableSetOf<String>()
    private val mutableStatus = MutableStateFlow<SyncStatus>(SyncStatus.Idle)
    val status: StateFlow<SyncStatus> = mutableStatus.asStateFlow()
    val cursor: StateFlow<SyncState?> = cache.sync
    /** R59: the snapshot has neither board cards nor agenda rows, so both projections are fetched completely first and
     * switch with the entities and cursor in ONE transaction. Any failed fetch switches nothing: the previous cache
     * stays and [status] reports the failure. [authoritative] (R81, stream resnapshot): replace without re-applying
     * responses received while the snapshot loaded; otherwise newer ones are re-applied (c2b N-1) unless the snapshot
     * predates the cursor installed at request time (restore/rollback). */
    internal var fetchListScopes: suspend (app.porchlight.shared.api.models.SnapshotRead) -> List<app.porchlight.shared.api.models.ListResponse>? = { null }
    private suspend fun refreshOnce(cohort: Cohort, onFailure: suspend () -> Unit) {
        var authoritative = false
        var resetCursor = false
        mutableStatus.value = SyncStatus.Refreshing
        overlapLock.withLock { snapshotInFlight = true; overlapped.clear() }
        suspend fun failed(status: SyncStatus) = withContext(NonCancellable) {
            // Failed status is a retry signal: retire the failed flight before publishing it to another caller.
            onFailure()
            mutableStatus.value = status
        }
        suspend fun read(admission: ReadCache.Fence) {
            snapshotStart(admission)
            mutex.withLock { cache.withFreshVisibility(admission, demandLock, onStarted = { visibility ->
                cohort.visibility = visibility
                authoritative = RefreshNeed.AuthoritativeSnapshot in cohort.needs
                resetCursor = RefreshNeed.ResetCursor in cohort.needs
            }) { fence ->
            call?.withReadSession { }
            val exclusive = targetingEnabled()
            if (exclusive) { targetedSlots.acquire(); try { targetedSlots.acquire() } catch (e: Exception) { targetedSlots.release(); throw e } }
            try {
            // Preserve this receipt horizon through a scoped ordering repair too: a newer provisional write
            // is not deletion evidence, even if the replacement snapshot still omits its representation.
            val mark = cache.snapshotMark()
            suspend fun load(fence: ReadCache.Fence) {
            if (exclusive) snapshotNotBefore = now() + 30.seconds
            val snapshot = api.readsSnapshot(version)
            val projectionAt = now()
            coroutineScope {
                val projections = async { fetchProjections(projectionAt) }
                val lists = async { projectionSlots.withPermit { fetchListScopes(snapshot) } }
                val retained = async {
                    if (!targetingEnabled()) emptyList() else coroutineScope {
                        agenda.retainedWindows().filterNot { window ->
                            window.memberId == snapshot.memberSettings.memberId &&
                                window.from >= projectionAt - 180.days && window.to <= projectionAt + 548.days
                        }.map { window -> async {
                            projectionSlots.withPermit { agenda.fetch(window.from, window.to, window.memberId) }
                        } }.awaitAll()
                    }
                }
                // Atomically with the switch, re-apply write responses newer than the snapshot's rows (undated too).
                val stagedProjections = projections.await()
                cache.installStaged(fence, snapshot, now(), stagedProjections, receivedAfter = mark.receipts, requestCursor = mark.cursor,
                    authoritative = authoritative, scopedChecks = lists.await(), preserveCursor = targetingEnabled() && !resetCursor,
                    retainedAgendas = retained.await())
            }
            }
            suspend fun orderedLoad() = cache.orderedRead { orderedFence -> load(orderedFence) }
            if (exclusive) cache.withFreshVisibility(fence) { orderedLoad() } else orderedLoad()
            } finally { if (exclusive) { targetedSlots.release(); targetedSlots.release() } }
            } }
        }
        try {
            try { read(cohort.admission) }
            catch (p: ApiProblem) {
                val wait = p.retryAfterMillis
                if (p.status !in setOf(429, 503) || wait == null) throw p
                // Only this staged read is retried, never its triggering mutation. Cancellation interrupts the wait.
                if (targetingEnabled()) {
                    snapshotNotBefore = maxOf(snapshotNotBefore ?: now(), now() + wait.coerceAtLeast(0).milliseconds)
                    waitForReadRetry(wait.coerceAtLeast(0))
                } else waitForReadRetry(wait.coerceIn(0L, 5000L))
                read(cohort.admission)
            }
            // Re-reads that ran meanwhile (write follow-ups, C4 tickles) are repeated once for the parent item, its
            // dated and undated occurrences and both projections. A failure reports Failed; the installed snapshot stays.
            val again = overlapLock.withLock { snapshotInFlight = false; overlapped.toSet().also { overlapped.clear() } }
            if (again.isNotEmpty()) { refetchItems(again); refreshProjections() }
            mutableStatus.value = SyncStatus.Ready
        } catch (e: CancellationException) { failed(SyncStatus.Idle); throw e }
        catch (e: HouseholdLocked) { failed(SyncStatus.Idle); throw e } // gated, not a connection failure
        catch (p: ApiProblem) {
            if (targetingEnabled() && p.status in setOf(429, 503)) p.retryAfterMillis?.let {
                snapshotNotBefore = maxOf(snapshotNotBefore ?: now(), now() + it.coerceAtLeast(0).milliseconds)
            }
            failed(SyncStatus.Failed(p.type)); throw p
        }
        catch (_: Exception) { failed(SyncStatus.Failed("connection-failed")); throw SyncUnavailable() }
        finally { withContext(NonCancellable) { overlapLock.withLock { snapshotInFlight = false; overlapped.clear() } } }
    }
    suspend fun onForeground() = refresh()
    /** The current board scope and the standard agenda window (the snapshot's 180-day past/18-month horizon). */
    private suspend fun fetchProjections(at: Instant = now()): Projections = coroutineScope {
        val cards = async { projectionSlots.withPermit { board.fetch() } }
        val rows = async { projectionSlots.withPermit { agenda.fetch(at - 180.days, at + 548.days) } }
        Projections(cards.await(), rows.await())
    }
    /** Refetches both projections, then replaces their rows in place together; a failure leaves both untouched. */
    suspend fun refreshProjections() = cache.orderedRead { fence -> cache.replaceProjections(fence, fetchProjections()) }
    /** C4's ID-only tickles use authoritative GETs. No occurrence GET exists: fetch its parent collection.
     * An unknown occurrence tickle requires a snapshot. 404 evicts rather than retaining hidden data. */
    suspend fun refetchByIds(itemIds: Set<String> = emptySet(), occurrenceIds: Set<String> = emptySet(),
                            memberIds: Set<String> = emptySet(), settingsChanged: Boolean = false) = cache.inSession { fence ->
        // Zone/authority derivation may change every affected occurrence, not just the member row.
        if (memberIds.isNotEmpty()) { refresh(); return@inSession }
        val byId = cache.view.value.occurrences.associateBy { it.id }
        if (occurrenceIds.any { it !in byId }) { refresh(); return@inSession }
        val parents = itemIds + occurrenceIds.mapNotNull { byId[it]?.itemId }
        refetchItems(parents)
        if (parents.isNotEmpty()) household.refreshMembers()
        if (settingsChanged) household.refreshSettings()
        if (parents.isNotEmpty()) refreshProjections()
    }
    /** Each item and its dated-window plus explicitly requested undated occurrences; 404 evicts. */
    private suspend fun refetchItems(ids: Set<String>) {
        overlapLock.withLock { if (snapshotInFlight) overlapped += ids }
        val at = now()
        var failure: ApiProblem? = null
        for (id in ids.sorted()) {
            try {
                items.get(id)
                occurrences.refresh(id, at - 180.days, at + 548.days, includeUndated = true)
            } catch (p: ApiProblem) {
                if (p.deniesVisibility()) cache.evictOnVisibilityFailure(id, p) else if (failure == null) failure = p
            }
        }
        failure?.let { throw it }
    }
    /** Occurrence write follow-up: the affected entities (parent item, its occurrences) and both projections in place. */
    internal suspend fun afterOccurrenceWrite(itemId: String) = cache.inSession { fence ->
        refetchItems(setOf(itemId))
        refreshProjections()
    }
    suspend fun evictItem(id: String) = cache.evictItem(id)
    // Never wait for a network-held refresh mutex to clear credentials' cached projections.
    suspend fun clear() {
        try { cache.clear(); mutableStatus.value = SyncStatus.Idle }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { mutableStatus.value = SyncStatus.Failed("attachment-cleanup-failed"); throw e }
    }
    /** Binding-owned retries keep the identity collector alive, without admitting a new session's reads first.
     * Explicit logout still uses clear(): its durable logout marker must survive failed file cleanup. */
    internal suspend fun awaitCleared() {
        while (true) {
            try { clear(); return }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { delay(1000) }
        }
    }
}
class SyncUnavailable : Exception("Household refresh failed; cached data remains available")

/** Downstream C3/C4 composition API. Owns no HTTP client; every generated call uses the trust coordinator. */
class HouseholdData(val cache: ReadCache, call: GeneratedCall, now: () -> Instant,
                    val writes: WriteCoordinator = WriteCoordinator(), version: String = BuildInfo.VERSION,
                    syncDispatcher: CoroutineDispatcher = Dispatchers.Default) {
    private val now = now
    // Bundles share inherited lists/suggestions with collection replacements. Keep that overlapping content
    // lane ordered, but never make unrelated inbox/settings reads wait for a stalled content request.
    private val targetedContentOrder = Mutex()
    private val targetedInboxOrder = Mutex()
    private val reads = ReadsApi(call)
    private val itemApi = ItemsApi(call)
    val household = HouseholdRepository(MembersApi(call), cache, writes, version)
    val items = ItemRepository(itemApi, cache, writes, version)
    internal var itemSyncCapability: () -> Boolean = { false }
    // Shipping kill switch: PrerolloutClientSyncDifferentialTest gates the item-sync-v1 rollout.
    internal var targetedSyncVerified = true
    internal val targetedSync: Boolean get() = itemSync && targetedSyncVerified
    val itemSync: Boolean get() = itemSyncCapability()
    val itemBundles = ItemSyncRepository(call, { itemSync }, version)
    val board = BoardRepository(reads, cache, version)
    val agenda = AgendaRepository(reads, cache, version)
    val occurrences = OccurrenceRepository(itemApi, cache, writes, board, version)
    val notifications = NotificationRepository(NotificationsApi(call), cache, writes, version)
    private var refreshAfterCacheRecovery: (suspend () -> Unit)? = null
    var localSnooze: (suspend (String) -> Unit)? = null
    val firedOccurrences = MutableStateFlow<Set<String>>(emptySet())
    var live: app.porchlight.shared.live.LiveSync? = null
        private set
    val exports = ExportRepository(call, cache)
    val imports = ImportRepository(call, writes, cache)
    val feeds = FeedRepository(call, writes, cache)
    val availability = AvailabilityRepository(call, writes, cache)
    val planning = PlanningRepository(call, writes, cache)
    val deviceAdmin = DeviceAdminRepository(call, writes, cache)
    val lists = ListRepository(call, cache, writes, version)
    val comments = CommentRepository(call, cache, writes, version)
    val attachments = AttachmentRepository(call, cache, writes, now)
    val search = SearchRepository(reads, version)
    val sync = SyncController(reads, cache, items, occurrences, household, agenda, board, now, version, call, syncDispatcher)
    init {
        // Item writes omit authoritative reminder IDs/scheduled reminders/creation metadata, which only the snapshot
        // carries: reconcile with a snapshot, staged with its projections (R59/R59a), so nothing is wiped meanwhile.
        // It queues behind any in-flight refresh, so it always installs a snapshot taken after the write.
        cache.sessionRevision = call::currentSessionRevision
        cache.writeOrigin = call::writeOrigin
        writes.writeOrigin = call::writeOrigin
        call.onSuccess = { live?.successfulRequest() }
        notifications.afterWrite = { sync.refresh() }
        notifications.scopedAfterWrite = { resource -> if (targetedSync) { sync.reconcile(SyncDemand(resource)); true } else false }
        feeds.afterWrite = { if (targetedSync) sync.reconcile(SyncDemand(SyncResource.Feeds)) else sync.refresh() }
        availability.scopedAfterWrite = { member -> if (targetedSync) { sync.reconcile(SyncDemand(SyncResource.Availability(member))); true } else false }
        lists.scopedAfterWrite = { id, occurrence ->
            if (targetedSync) {
                sync.reconcile(SyncDemand(id?.let { SyncResource.List(it) } ?: SyncResource.Lists, listScopes = setOfNotNull(occurrence)))
                true
            } else false
        }
        comments.scopedAfterWrite = { item, id ->
            if (targetedSync) { sync.reconcile(SyncDemand(SyncResource.Comment(item, id))); true } else false
        }
        attachments.scopedReadsEnabled = { targetedSync }
        attachments.scopedAfterWrite = { item -> if (targetedSync) { sync.reconcile(itemDemand(item, true)); true } else false }
        planning.scopedAfterWrite = { item ->
            if (targetedSync) {
                val demands = listOfNotNull(SyncDemand(SyncResource.Suggestions), item?.let { itemDemand(it) })
                sync.reconcile(*demands.toTypedArray()); true
            } else false
        }
        availability.afterWrite = { sync.refresh() }
        imports.afterCommit = { sync.refresh() }
        planning.afterAccept = { sync.refresh() }
        sync.targetingEnabled = { targetedSync }
        sync.targetedRead = { fence, demand -> readTargeted(fence, demand) }
        sync.targetedOrder = { demand, operation ->
            when (demand.resource) {
                is SyncResource.Item, is SyncResource.Comment, is SyncResource.List,
                SyncResource.Lists, SyncResource.Suggestions -> targetedContentOrder.withLock { operation() }
                is SyncResource.Notification, SyncResource.Inbox -> targetedInboxOrder.withLock { operation() }
                else -> operation()
            }
        }
        sync.targetedFollowUp = { demand ->
            val resource = demand.resource
            if (resource is SyncResource.Item && resource.id in cache.view.value.evictedItemIds)
                sync.reconcile(SyncDemand(SyncResource.Inbox))
        }
        items.afterWrite = { id -> if (targetedSync && id != null) sync.reconcile(itemDemand(id)) else sync.refresh() }
        items.afterSplitWrite = { id -> if (targetedSync) sync.reconcile(itemDemand(id, true)) }
        items.afterDeleteWrite = { id -> if (targetedSync) sync.reconcile(itemDemand(id)) }
        items.afterUnknownSplitWrite = { if (targetedSync) { sync.refresh(authoritative = true); true } else false }
        items.afterFirstVisibilityWrite = { id -> if (targetedSync) { sync.reconcile(itemDemand(id, true)); true } else false }
        household.afterSettingsWrite = { if (targetedSync) sync.reconcile(SyncDemand(SyncResource.Today)) }
        household.afterMemberWrite = { previous, member ->
            if (targetedSync) {
                if (previous == null || previous.role != member.role) sync.refresh(authoritative = true)
                else {
                    sync.reconcile(SyncDemand(SyncResource.Members))
                    if (previous.tzid != member.tzid) {
                        val affected = cache.view.value.items.filter { it.ownerId == member.id || member.id in it.assignees }
                        sync.reconcile(*affected.map { itemDemand(it.id) }.toTypedArray())
                    }
                }
            }
        }
        sync.fetchListScopes = { lists.fetchRetainedScopes(it) }
        lists.afterWrite = { sync.refresh() }; comments.afterWrite = { sync.refresh() }; attachments.afterWrite = { sync.refresh() }
        occurrences.afterWrite = { id -> if (targetedSync) sync.reconcile(itemDemand(id)) else sync.afterOccurrenceWrite(id) }
        occurrences.afterUnknownWrite = { if (targetedSync) { sync.refresh(authoritative = true); true } else false }
    }
    /** Application lifetime, not composition lifetime; cancellation prevents a late old-session sync promotion. */
    fun bindTrust(controller: TrustController, scope: CoroutineScope, onIdentityEpoch: ((Long) -> Unit)? = null, onIdentityEnded: suspend (Long) -> Unit = {},
        streamEnabled: StateFlow<Boolean> = MutableStateFlow(true), refreshWhenInactive: Boolean = true, resumeFromCursor: Boolean = false,
        onIdentityStarting: suspend (CacheOwner, Long) -> Unit = { _, _ -> }): Job {
        itemSyncCapability = { controller.itemSync }
        val source = app.porchlight.shared.live.StreamSource { after, receive ->
            if (!controller.backgroundSessionUsable()) throw HouseholdLocked()
            val epoch = controller.sessionEpoch.value
            val token = controller.sessionToken()
            try {
                controller.transport().stream(after, token, controller.itemSync) { frame ->
                    if (epoch != controller.sessionEpoch.value || token != controller.sessionToken()) throw CancellationException("Household session changed")
                    if (!controller.backgroundSessionUsable()) throw HouseholdLocked()
                    receive(frame)
                }
            } catch (p: ApiProblem) { if (epoch == controller.sessionEpoch.value && token == controller.sessionToken()) controller.onProblem(p); throw p }
            catch (e: app.porchlight.shared.transport.DeviceCertificateRevoked) { if (epoch == controller.sessionEpoch.value) controller.onTransportError(e); throw e }
        }
        live = app.porchlight.shared.live.LiveSync(source, { cache.sync.value?.cursor }, { frame ->
            applyStreamFrame(frame)
        }, { seq -> cache.inSession { cache.acknowledgeCursor(seq) } },
            { sync.resnapshot() }, now, concurrentApply = { targetedSync }).also {
            it.progressBlocked = { cache.cleanupRetryAt.value != null }
        }
        cache.ownerBindingRequired = true
        cache.sessionEpoch = { controller.sessionEpoch.value }
        cache.sessionCommitMutex = controller.sessionCommitMutex
        return bindIdentity(controller, scope, onIdentityEpoch, onIdentityEnded, streamEnabled, refreshWhenInactive, resumeFromCursor, onIdentityStarting)
    }
    internal suspend fun applyStreamFrame(frame: app.porchlight.shared.live.StreamFrame.Tickle) {
        if (targetedSync) cache.inSession { applyTargeted(frame) }
        else cache.inSession {
                when {
                    frame.evicted && frame.entityType == "item" -> sync.evictItem(frame.entityId)
                    frame.kind == "item.changed" || frame.kind == "item.access_changed" -> sync.refreshForEvent(frame.seq)
                    frame.kind == "occurrence.changed" -> sync.refetchByIds(occurrenceIds = setOf(frame.entityId))
                    // Only the snapshot carries scheduled reminder identities, so reconcile them atomically too.
                    frame.kind == "derived" -> sync.refreshForEvent(frame.seq)
                    frame.kind.startsWith("notification.") -> notifications.refresh()
                    frame.entityType == "list" -> lists.refetchByIds(setOf(frame.entityId))
                    frame.entityType == "list_entry" -> {
                        val ids = cache.view.value.lists.filter { row -> (row.propertyEntries + row.gotIt).any { it.id == frame.entityId } }.mapTo(mutableSetOf()) { it.id }
                        if (ids.isEmpty()) sync.refreshForEvent(frame.seq) else lists.refetchByIds(ids)
                    }
                    // Member/private and other collaboration entities need a snapshot.
                    else -> sync.refreshForEvent(frame.seq)
                }
        }
    }
    private fun bindIdentity(controller: TrustController, scope: CoroutineScope, onIdentityEpoch: ((Long) -> Unit)?,
        onIdentityEnded: suspend (Long) -> Unit, streamEnabled: StateFlow<Boolean>, refreshWhenInactive: Boolean, resumeFromCursor: Boolean,
        onIdentityStarting: suspend (CacheOwner, Long) -> Unit): Job {
        // Install before restore: logout's durable marker survives failed/closed-cache clears and
        // restores this obligation on next start. Cleanup and owner admission share this binding-local lock.
        val cleanupMutex = Mutex()
        val admissionFailed = MutableStateFlow(false)
        var cleanedEpoch: Long? = null
        var boundEpoch: Long? = null
        suspend fun clearIdentityLocked(expected: Long) {
            if (expected != controller.sessionEpoch.value || cleanedEpoch == expected || boundEpoch == expected) return
            sync.clear()
            if (expected != controller.sessionEpoch.value) return
            onIdentityEnded(expected)
            if (expected == controller.sessionEpoch.value) {
                cleanedEpoch = expected
                controller.identityCleanupCompleted(expected)
            }
        }
        suspend fun clearIdentity(expected: Long) = cleanupMutex.withLock { clearIdentityLocked(expected) }
        controller.recoverLocalCache = { cleanupMutex.withLock { cache.clearLocalCache() } }
        controller.retryCacheCleanup = { cleanupMutex.withLock { cache.allowCleanupRetryNow() } }
        controller.identityStarting = { value -> cleanupMutex.withLock {
            try {
                // A failed native clear is still an admission obligation even if owner reconciliation could succeed.
                val epoch = controller.sessionEpoch.value
                if (controller.identityCleanupRequired(epoch)) clearIdentityLocked(epoch)
                cache.initialize(value.cacheOwner())
                // Native inventories have their own durable owner: a prior process may have cleared only SQL/files.
                onIdentityStarting(value.cacheOwner(), epoch)
                boundEpoch = epoch
                admissionFailed.value = false
            } catch (e: Exception) { admissionFailed.value = true; throw e }
        } }
        suspend fun awaitIdentityCleared(expected: Long) {
            while (expected == controller.sessionEpoch.value) {
                try { clearIdentity(expected); return }
                catch (e: CancellationException) { throw e }
                catch (_: Exception) { delay(1000) }
            }
        }
        // A synchronous attempt is optional: cancellation/failure leaves the epoch obligation for this binding's retry.
        controller.identityEnded = { clearIdentity(controller.sessionEpoch.value) }
        var previousEpoch = controller.sessionEpoch.value
        var notifiedEpoch: Long? = null
        var targetedSession: Pair<Long, Long>? = null
        return scope.launch {
            launch {
                cache.cleanupRetryAt.collectLatest { retryAt ->
                    live?.tick()
                    if (retryAt != null) {
                        delay((retryAt - now()).inWholeMilliseconds.coerceAtLeast(0))
                        try { cache.retryAttachmentCleanup(); live?.networkChanged() }
                        catch (e: CancellationException) { throw e }
                        catch (_: Exception) { /* The durable next deadline schedules the next attempt. */ }
                    }
                }
            }
            // A failed owner bind is a gate, not a Ready UI with a background-only retry.
            launch {
                while (currentCoroutineContext().isActive) {
                    combine(controller.state, cache.storageNotice, admissionFailed) { state, notice, failed ->
                        state == TrustState.StorageRecoveryRequired && (notice != null || failed)
                    }.first { it }
                    delay(1000)
                    if (controller.state.value == TrustState.StorageRecoveryRequired && (cache.storageNotice.value != null || admissionFailed.value))
                        controller.restore()
                }
            }
            launch { controller.state.collect { if (it == TrustState.Ready) controller.refreshWhenReady() } }
            val protocolRevision = combine(controller.sessionRevision, controller.itemSyncSupport) { revision, support -> revision to support }
            combine(controller.state, controller.sessionEpoch, protocolRevision, streamEnabled, cache.storageRevision) { state, epoch, revision, enabled, storage ->
                // Ready <-> PinRequired is the SAME session: do not cancel/refetch/reconnect on a UI relock.
                val active = state == TrustState.Ready || state == TrustState.PinRequired
                // Storage loss restarts an admitted stream, not the ownerless clear that emitted it.
                Triple(Triple(if (state == TrustState.PinRequired) TrustState.Ready else state, epoch, revision), enabled, storage.takeIf { active })
            }.distinctUntilChanged().collectLatest { (session, enabled, _) ->
                val (state, epoch, protocol) = session
                sync.refreshSessionChanged(controller.sessionEpoch.value, cache.storageRevision.value, controller.sessionRevision.value)
                val currentTargetedSession = epoch to protocol.first
                if (targetedSession != currentTargetedSession) {
                    sync.targetedSessionChanged(); targetedSession = currentTargetedSession
                }
                if (notifiedEpoch != epoch) {
                    onIdentityEpoch?.let { callback -> withContext(Dispatchers.Main.immediate) { callback(epoch) } }
                    notifiedEpoch = epoch
                }
                // Retry the whole epoch obligation, but never repeat a completed clear after replacement admission.
                if (epoch != previousEpoch) { awaitIdentityCleared(epoch); previousEpoch = epoch }
                when (state) {
                    TrustState.Ready -> try { withBackgroundSession(controller) {
                        controller.busy.first { !it }
                        if (controller.backgroundSessionUsable()) {
                            if ((enabled || refreshWhenInactive) && (!resumeFromCursor || cache.sync.value == null)) try { sync.refresh() } catch (e: CancellationException) { throw e } catch (_: Exception) { /* live reconnect repairs an unavailable first snapshot */ }
                            if (enabled) { sync.awaitAdmittedRefreshes(); live?.run() }
                        }
                    } } catch (e: CancellationException) {
                        currentCoroutineContext().ensureActive() // a fenced stale-session reply is not our cancellation
                    } catch (_: Exception) { /* status is redacted */ }
                    // No live identity owns the data: covers a process that died before an earlier clear committed.
                    TrustState.Enrollment, TrustState.ReEnroll -> { awaitIdentityCleared(epoch) }
                    // Login (cold start after expiry/logout), SessionExpired, PIN, wallet, upgrade and local-trust retry
                    // keep the persisted cache, hidden behind the gate, for the same identity.
                    else -> Unit
                }
            }
        }
    }
    suspend fun clearLocalCache() {
        cache.clearLocalCache()
        sync.refresh(authoritative = true)
        live?.networkChanged()
    }
    fun close() { sync.close(); cache.close() }
}
