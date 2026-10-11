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
import kotlin.time.Duration.Companion.days
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
                     private val version: String = BuildInfo.VERSION) {
    private val mutex = Mutex()
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
    suspend fun refresh(authoritative: Boolean = false) = cache.inSession { fence -> mutex.withLock {
        mutableStatus.value = SyncStatus.Refreshing
        overlapLock.withLock { snapshotInFlight = true; overlapped.clear() }
        try {
            // Before the request: later responses may be newer than this snapshot. The cursor installed now (not at
            // install time, which stream acks may have advanced) is the basis for restore detection (R81, N-1).
            val mark = cache.snapshotMark()
            val snapshot = api.readsSnapshot(version)
            val projections = fetchProjections()
            // Atomically with the switch, re-apply write responses newer than the snapshot's rows (undated too).
            cache.installStaged(fence, snapshot, now(), projections, receivedAfter = mark.receipts, requestCursor = mark.cursor,
                authoritative = authoritative, scopedChecks = fetchListScopes(snapshot))
            // Re-reads that ran meanwhile (write follow-ups, C4 tickles) are repeated once for the parent item, its
            // dated and undated occurrences and both projections. A failure reports Failed; the installed snapshot stays.
            val again = overlapLock.withLock { snapshotInFlight = false; overlapped.toSet().also { overlapped.clear() } }
            if (again.isNotEmpty()) { refetchItems(again); refreshProjections() }
            mutableStatus.value = SyncStatus.Ready
        } catch (e: CancellationException) { mutableStatus.value = SyncStatus.Idle; throw e }
        catch (e: HouseholdLocked) { mutableStatus.value = SyncStatus.Idle; throw e } // gated, not a connection failure
        catch (p: ApiProblem) { mutableStatus.value = SyncStatus.Failed(p.type); throw p }
        catch (_: Exception) { mutableStatus.value = SyncStatus.Failed("connection-failed"); throw SyncUnavailable() }
        finally { withContext(NonCancellable) { overlapLock.withLock { snapshotInFlight = false; overlapped.clear() } } }
    }
    }
    suspend fun onForeground() = refresh()
    /** The current board scope and the standard agenda window (the snapshot's 180-day past/18-month horizon). */
    private suspend fun fetchProjections(): Projections {
        val at = now()
        return Projections(board.fetch(), agenda.fetch(at - 180.days, at + 548.days))
    }
    /** Refetches both projections, then replaces their rows in place together; a failure leaves both untouched. */
    suspend fun refreshProjections() = cache.inSession { fence -> cache.replaceProjections(fence, fetchProjections()) }
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
                    val writes: WriteCoordinator = WriteCoordinator(), version: String = BuildInfo.VERSION) {
    private val now = now
    private val reads = ReadsApi(call)
    private val itemApi = ItemsApi(call)
    val household = HouseholdRepository(MembersApi(call), cache, writes, version)
    val items = ItemRepository(itemApi, cache, writes, version)
    val board = BoardRepository(reads, cache, version)
    val agenda = AgendaRepository(reads, cache, version)
    val occurrences = OccurrenceRepository(itemApi, cache, writes, board, version)
    val notifications = NotificationRepository(NotificationsApi(call), cache, writes, version)
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
    val sync = SyncController(reads, cache, items, occurrences, household, agenda, board, now, version)
    init {
        // Item writes omit authoritative reminder IDs/scheduled reminders/creation metadata, which only the snapshot
        // carries: reconcile with a snapshot, staged with its projections (R59/R59a), so nothing is wiped meanwhile.
        // It queues behind any in-flight refresh, so it always installs a snapshot taken after the write.
        call.onSuccess = { live?.successfulRequest() }
        notifications.afterWrite = { sync.refresh() }
        feeds.afterWrite = { sync.refresh() }
        availability.afterWrite = { sync.refresh() }
        imports.afterCommit = { sync.refresh() }
        planning.afterAccept = { sync.refresh() }
        items.afterWrite = { _ -> sync.refresh() }
        sync.fetchListScopes = { lists.fetchRetainedScopes(it) }
        lists.afterWrite = { sync.refresh() }; comments.afterWrite = { sync.refresh() }; attachments.afterWrite = { sync.refresh() }
        occurrences.afterWrite = { id -> sync.afterOccurrenceWrite(id) }
    }
    /** Application lifetime, not composition lifetime; cancellation prevents a late old-session sync promotion. */
    fun bindTrust(controller: TrustController, scope: CoroutineScope, onIdentityEpoch: ((Long) -> Unit)? = null, onIdentityEnded: suspend (Long) -> Unit = {},
        streamEnabled: StateFlow<Boolean> = MutableStateFlow(true), refreshWhenInactive: Boolean = true, resumeFromCursor: Boolean = false,
        onIdentityStarting: suspend (CacheOwner, Long) -> Unit = { _, _ -> }): Job {
        val source = app.porchlight.shared.live.StreamSource { after, receive ->
            if (!controller.backgroundSessionUsable()) throw HouseholdLocked()
            val epoch = controller.sessionEpoch.value
            val token = controller.sessionToken()
            try {
                controller.transport().stream(after, token) { frame ->
                    if (epoch != controller.sessionEpoch.value || token != controller.sessionToken()) throw CancellationException("Household session changed")
                    if (!controller.backgroundSessionUsable()) throw HouseholdLocked()
                    receive(frame)
                }
            } catch (p: ApiProblem) { if (epoch == controller.sessionEpoch.value && token == controller.sessionToken()) controller.onProblem(p); throw p }
            catch (e: app.porchlight.shared.transport.DeviceCertificateRevoked) { if (epoch == controller.sessionEpoch.value) controller.onTransportError(e); throw e }
        }
        live = app.porchlight.shared.live.LiveSync(source, { cache.sync.value?.cursor }, { frame ->
            cache.inSession { fence ->
                when {
                    frame.evicted && frame.entityType == "item" -> sync.evictItem(frame.entityId)
                    frame.kind == "item.changed" || frame.kind == "item.access_changed" -> sync.refresh()
                    frame.kind == "occurrence.changed" -> sync.refetchByIds(occurrenceIds = setOf(frame.entityId))
                    // Only the snapshot carries scheduled reminder identities, so reconcile them atomically too.
                    frame.kind == "derived" -> sync.refresh()
                    frame.kind.startsWith("notification.") -> notifications.refresh()
                    frame.entityType == "list" -> lists.refetchByIds(setOf(frame.entityId))
                    frame.entityType == "list_entry" -> {
                        val ids = cache.view.value.lists.filter { row -> (row.propertyEntries + row.gotIt).any { it.id == frame.entityId } }.mapTo(mutableSetOf()) { it.id }
                        if (ids.isEmpty()) sync.refresh() else lists.refetchByIds(ids)
                    }
                    // Member/private and other collaboration entities need a snapshot.
                    else -> sync.refresh()
                }
            }
        }, { seq -> cache.inSession { fence -> cache.acknowledgeCursor(seq) } },
            // R81: a terminal `resnapshot` frame (cursor expired / epoch changed) replaces the cache authoritatively,
            // without re-applying older receipts. Also used for the cursor-less start, which has no receipts to keep.
            { sync.refresh(authoritative = true) }, now)
        cache.ownerBindingRequired = true
        cache.sessionEpoch = { controller.sessionEpoch.value }
        cache.sessionCommitMutex = controller.sessionCommitMutex
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
        return scope.launch {
            // A failed owner bind is a gate, not a Ready UI with a background-only retry.
            launch {
                while (currentCoroutineContext().isActive) {
                    combine(controller.state, cache.storageNotice, admissionFailed) { state, notice, failed ->
                        state is TrustState.LocalTrustLost && (notice != null || failed)
                    }.first { it }
                    delay(1000)
                    if (controller.state.value is TrustState.LocalTrustLost && (cache.storageNotice.value != null || admissionFailed.value))
                        controller.restore()
                }
            }
            launch { controller.state.collect { if (it == TrustState.Ready) controller.refreshWhenReady() } }
            combine(controller.state, controller.sessionEpoch, controller.sessionRevision, streamEnabled, cache.storageRevision) { state, epoch, revision, enabled, storage ->
                // Ready <-> PinRequired is the SAME session: do not cancel/refetch/reconnect on a UI relock.
                val active = state == TrustState.Ready || state == TrustState.PinRequired
                // Storage loss restarts an admitted stream, not the ownerless clear that emitted it.
                Triple(Triple(if (state == TrustState.PinRequired) TrustState.Ready else state, epoch, revision), enabled, storage.takeIf { active })
            }.distinctUntilChanged().collectLatest { (session, enabled, _) ->
                val (state, epoch, _) = session
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
                            if (enabled) live?.run()
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
    fun close() = cache.close()
}
