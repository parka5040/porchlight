package app.porchlight.shared.data

import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncSchedulerTest {
    private val f = SyncBundleFixtures
    private fun item(id: String = "item") = SyncDemand(SyncResource.Item(id))

    @Test fun burstDeduplicatesAndUnionsHydrationBeforeReadStart() = runTest {
        val cache = memoryCache { f.now }; cache.initialize()
        val admissions = Channel<Unit>(Channel.UNLIMITED)
        val reads = mutableListOf<SyncDemand>()
        val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { _, demand -> reads += demand; false }, {})
        scheduler.admitted = { admissions.trySend(Unit) }
        try {
            val demands = (1..12).map { index -> async { scheduler.request(listOf(item().copy(collaboration = index == 12))) } }
            repeat(12) { admissions.receive() }
            assertTrue(reads.isEmpty())
            advanceTimeBy(249); runCurrent(); assertTrue(reads.isEmpty())
            advanceTimeBy(1)
            demands.awaitAll()
            assertEquals(1, reads.size)
            assertTrue(reads.single().collaboration)
        } finally { cache.close() }
    }

    @Test fun lateEventGetsItsOwnCohortAndFirstWaiterDoesNotAwaitIt() = runTest {
        val cache = memoryCache { f.now }; cache.initialize()
        val started = Channel<CompletableDeferred<Unit>>(Channel.UNLIMITED)
        val admissions = Channel<Unit>(Channel.UNLIMITED)
        val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { _, _ ->
            CompletableDeferred<Unit>().also { started.send(it) }.await(); false
        }, {})
        scheduler.admitted = { admissions.trySend(Unit) }
        try {
            val first = async { scheduler.request(listOf(item())) }; admissions.receive()
            val releaseFirst = started.receive()
            val late = async { scheduler.request(listOf(item())) }; admissions.receive()
            releaseFirst.complete(Unit); first.await()
            assertFalse(late.isCompleted)
            started.receive().complete(Unit); late.await()
        } finally { cache.close() }
    }

    @Test fun twoReadCeilingAndPerResourceFailureIsolationAndExplicitRetry() = runTest {
        val cache = memoryCache { f.now }; cache.initialize()
        val started = Channel<Pair<SyncResource, CompletableDeferred<Unit>>>(Channel.UNLIMITED)
        var active = 0; var peak = 0; var fail = true
        val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { _, demand ->
            active++; peak = maxOf(peak, active)
            try {
                CompletableDeferred<Unit>().also { started.send(demand.resource to it) }.await()
                if (demand.resource == SyncResource.Item("bad") && fail) error("fixture read failure")
                false
            } finally { active-- }
        }, {})
        try {
            val bad = async { runCatching { scheduler.request(listOf(item("bad"))) } }
            val good = async { scheduler.request(listOf(item("good"))) }
            val third = async { scheduler.request(listOf(item("third"))) }
            val a = started.receive(); val b = started.receive()
            assertEquals(2, active)
            a.second.complete(Unit); b.second.complete(Unit)
            started.receive().second.complete(Unit)
            good.await(); third.await(); assertTrue(bad.await().isFailure)
            assertEquals(2, peak)
            fail = false
            val retry = async { scheduler.request(listOf(item("bad"))) }
            started.receive().second.complete(Unit); retry.await()
        } finally { cache.close() }
    }

    @Test fun cancellingWaiterDoesNotCancelApplicationOwnedRead() = runTest {
        val cache = memoryCache { f.now }; cache.initialize()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>(); val installed = CompletableDeferred<Unit>()
        val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { _, _ ->
            started.complete(Unit); release.await(); installed.complete(Unit); false
        }, {})
        try {
            val waiter = launch { scheduler.request(listOf(item())) }; started.await()
            waiter.cancelAndJoin(); release.complete(Unit); installed.await()
        } finally { cache.close() }
    }
}
