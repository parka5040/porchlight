package app.porchlight.shared.data

import kotlinx.coroutines.*
import app.porchlight.shared.transport.ApiProblem
import kotlin.time.Clock
import kotlin.time.Instant
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.withPermit
import kotlin.coroutines.AbstractCoroutineContextElement
import kotlin.coroutines.ContinuationInterceptor
import kotlin.coroutines.CoroutineContext

internal sealed interface SyncResource {
    data class Item(val id: String) : SyncResource
    data class Notification(val id: String) : SyncResource
    data class Comment(val itemId: String, val id: String) : SyncResource
    data class List(val id: String) : SyncResource
    data class Availability(val memberId: String) : SyncResource
    data object Members : SyncResource
    data object Today : SyncResource
    data object NotificationPrefs : SyncResource
    data object Inbox : SyncResource
    data object Lists : SyncResource
    data object Suggestions : SyncResource
    data object Feeds : SyncResource
}

internal data class SyncDemand(val resource: SyncResource, val collaboration: Boolean = false,
                               val agenda: Set<ItemAgendaWindow> = emptySet(), val listScopes: Set<String> = emptySet()) {
    fun merge(other: SyncDemand) = copy(collaboration = collaboration || other.collaboration,
        agenda = agenda + other.agenda, listScopes = listScopes + other.listScopes)
}

/** Covers every continuation/scoped GET, including repositories that continue after one scope fails. */
internal class TargetedReadPolicy(val before: suspend () -> Unit, private val failed: suspend (ApiProblem) -> Unit) : AbstractCoroutineContextElement(Key) {
    companion object Key : CoroutineContext.Key<TargetedReadPolicy>
    val recorded = mutableSetOf<ApiProblem>()
    suspend fun record(problem: ApiProblem) { if (recorded.add(problem)) failed(problem) }
}

/** Coverage belongs to a resource/read-start cohort, never a whole delivered batch. */
internal class TargetedSyncScheduler(
    private val cache: ReadCache, private val call: GeneratedCall?, private val scope: CoroutineScope,
    private val slots: Semaphore, private val read: suspend (ReadCache.Fence, SyncDemand) -> Boolean,
    private val snapshot: suspend () -> Unit, private val now: () -> Instant = { Clock.System.now() },
) {
    private data class Key(val epoch: Long, val storage: Long, val revision: Long, val resource: SyncResource)
    private class Cohort(val context: CoroutineContext, val fence: ReadCache.Fence, var demand: SyncDemand) {
        var started = false
        val result = CompletableDeferred<Unit>()
    }
    private class Flight(val active: Cohort) { var pending: Cohort? = null }
    private val lock = Mutex()
    private val flights = mutableMapOf<Key, Flight>()
    private var cooldownSession: Triple<Long, Long, Long>? = null
    private var notBefore: Instant? = null
    private var cooldownChanged = CompletableDeferred<Unit>()
    private val jobs = mutableMapOf<Key, Job>()
    private class CooldownPending : CancellationException()
    private fun replaceSession(session: Triple<Long, Long, Long>?) {
        cooldownSession = session; notBefore = null
        cooldownChanged.complete(Unit); cooldownChanged = CompletableDeferred()
        val stale = flights.keys.filter { it.session() != session }
        for (key in stale) {
            val cancelled = CancellationException("Targeted session replaced")
            jobs.remove(key)?.cancel(cancelled)
            flights.remove(key)?.let { flight ->
                flight.active.result.completeExceptionally(cancelled)
                flight.pending?.result?.completeExceptionally(cancelled)
            }
        }
    }
    suspend fun sessionChanged() = lock.withLock { replaceSession(null) }
    private fun Key.session() = Triple(epoch, storage, revision)
    private suspend fun awaitReadDeadline(key: Key) {
        while (true) {
            val (deadline, changed) = lock.withLock {
                if (cooldownSession != key.session()) throw CancellationException("Targeted session replaced")
                (notBefore ?: return) to cooldownChanged
            }
            val wait = (deadline - now()).inWholeMilliseconds.coerceAtLeast(0)
            if (wait > 0) {
                waitingForCooldown()
                withTimeoutOrNull(wait) { changed.await() }
            }
            // Completing the cancellable duration also expires the deadline if the wall clock stood still
            // or moved backwards. A concurrent overload extending it still requires another wait.
            val expired = lock.withLock {
                if (cooldownSession != key.session()) throw CancellationException("Targeted session replaced")
                else if (notBefore == deadline) { notBefore = null; true } else false
            }
            if (expired) return
        }
    }
    private suspend fun checkReadAdmission(key: Key) = lock.withLock {
        if (cooldownSession != key.session()) throw CancellationException("Targeted session replaced")
        if (notBefore?.let { it > now() } == true) throw CooldownPending()
    }
    private suspend fun recordOverload(key: Key, problem: ApiProblem) {
        if (problem.status !in setOf(429, 503)) return
        val retry = problem.retryAfterMillis?.coerceIn(0, 300_000) ?: return
        lock.withLock {
            if (cooldownSession == key.session()) {
                val deadline = now() + retry.milliseconds
                notBefore = maxOf(notBefore ?: deadline, deadline)
            }
        }
    }
    internal var batchMillis = 250L
    internal var waitForBatch: suspend (Long) -> Unit = { delay(it) }
    internal var admitted: () -> Unit = {}
    internal var waitingForCooldown: () -> Unit = {}
    internal var orderRead: suspend (SyncDemand, suspend () -> Boolean) -> Boolean = { _, operation -> operation() }
    internal var afterRead: suspend (SyncDemand) -> Unit = {}

    suspend fun request(demands: Collection<SyncDemand>) {
        if (call == null) admit(demands, 0) else call.withReadSession { admit(demands, it) }
    }
    private suspend fun admit(demands: Collection<SyncDemand>, revision: Long) = cache.inSession { fence ->
        val lease = cache.refreshKey(fence)
        val context = currentCoroutineContext().minusKey(Job).minusKey(ContinuationInterceptor)
        val waiting = lock.withLock {
            scope.coroutineContext.ensureActive()
            val session = Triple(lease.first, lease.second, revision)
            if (cooldownSession != session) replaceSession(session)
            demands.map { demand ->
                val key = Key(lease.first, lease.second, revision, demand.resource)
                val flight = flights[key]
                // Reconnect replays a rejected demand; never acknowledge overflow or retain an unbounded ID queue.
                if (flight == null && flights.size >= 512) throw SyncUnavailable()
                val cohort = when {
                    flight == null -> Cohort(context, fence, demand).also { cohort ->
                        val created = Flight(cohort)
                        flights[key] = created
                        jobs[key] = scope.launch { drain(key, created) }.also { job -> job.invokeOnCompletion { failure ->
                            if (failure != null) cohort.result.completeExceptionally(failure)
                        } }
                    }
                    !flight.active.started -> flight.active.also { it.demand = it.demand.merge(demand) }
                    else -> flight.pending?.also { it.demand = it.demand.merge(demand) }
                        ?: Cohort(context, fence, demand).also { flight.pending = it }
                }
                cohort.result
            }
        }
        admitted()
        // Await all independently: a failure must not cancel unrelated resource work or grant its coverage.
        var failure: Exception? = null
        for (result in waiting) try { result.await() }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (failure == null) failure = e }
        failure?.let { throw it }
    }
    private suspend fun readCohort(key: Key, cohort: Cohort): Boolean {
        var repairs = 0
        while (true) {
            var visibility = Long.MAX_VALUE
            var overload: ApiProblem? = null
            try {
                awaitReadDeadline(key)
                return orderRead(cohort.demand) {
                    slots.withPermit {
                        checkReadAdmission(key)
                        // A new cooldown during pagination unwinds the lane/permit before waiting and
                        // retries the cohort with fresh coverage; no scarce resource sleeps on overload.
                        val policy = TargetedReadPolicy({ checkReadAdmission(key) }, { overload = it; recordOverload(key, it) })
                        try {
                            withContext(policy) {
                                cache.withFreshVisibility(cohort.fence, lock, onStarted = {
                                    cohort.started = true; visibility = it
                                }) {
                                    cache.orderedRead { orderedFence -> read(orderedFence, cohort.demand) }
                                }
                            }
                        } catch (p: ApiProblem) { if (p !in policy.recorded) recordOverload(key, p); throw p }
                    }
                }
            } catch (_: CooldownPending) {
                // If this cohort already failed, leave it unacknowledged for replay rather than
                // sleeping through remaining scopes (or automatically replaying failed HTTP reads).
                overload?.let { throw it }
                currentCoroutineContext().ensureActive()
            } catch (e: StaleVisibilityReply) {
                // Only scheduled repairs may renew a crossed privacy fence, and only by reading again.
                // Preserve identity/storage/session admission and bound churn from continuing evictions.
                if (cache.view.value.visibilityRevision <= visibility || repairs++ >= 3) throw e
                currentCoroutineContext().ensureActive()
            }
        }
    }
    private suspend fun drain(key: Key, initial: Flight) {
        val drainingJob = currentCoroutineContext()[Job]
        var flight = initial
        try {
            while (true) {
                waitForBatch(batchMillis)
                val cohort = flight.active
                var failure: Exception? = null
                try {
                    withContext(cohort.context) {
                        // An overlapping collection waits outside the shared slots, and captures its fence only
                        // when its own network work can start. Independent resources can still use the other slot.
                        val needsSnapshot = readCohort(key, cohort)
                        // Never hold a targeted slot while waiting for the exclusive snapshot fallback.
                        if (needsSnapshot) snapshot() else afterRead(cohort.demand)
                    }
                } catch (e: Exception) { failure = e; currentCoroutineContext().ensureActive() }
                val next = lock.withLock {
                    val pending = flight.pending
                    if (pending == null) { flights.remove(key); null }
                    else Flight(pending).also { flights[key] = it }
                }
                if (failure == null) cohort.result.complete(Unit) else cohort.result.completeExceptionally(failure)
                if (next == null) return
                flight = next
            }
        } finally {
            withContext(NonCancellable) { lock.withLock {
                if (flights[key] === flight) flights.remove(key)
                if (jobs[key] === drainingJob) jobs.remove(key)
                val stopped = CancellationException("Targeted reconciliation stopped")
                flight.active.result.completeExceptionally(stopped)
                flight.pending?.result?.completeExceptionally(stopped)
            } }
        }
    }
}
