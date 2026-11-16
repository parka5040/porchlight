package app.porchlight.shared.data

import app.porchlight.shared.live.*
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Duration.Companion.milliseconds

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncFixBRetryTest {
    private val f = SyncBundleFixtures

    @Test fun replayHonoursTargetedRetryAfterAcrossResourcesAndRecovers() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this)
        val starts = mutableListOf<Long>()
        var fail = true
        h.data.sync.waitForTargetedBatch = { }
        h.before = { _, _ ->
            starts += testScheduler.currentTime
            if (fail) { fail = false; throw ApiProblem("rate-limited", 429, "Retry", retryAfterMillis = 60_000) }
        }
        try {
            h.seed()
            assertFailsWith<ApiProblem> { h.data.sync.reconcile(SyncDemand(SyncResource.Item(f.snapshot.items.single().id))) }
            val replay = async { h.data.sync.reconcile(SyncDemand(SyncResource.Notification("notice"))) }
            replay.await()
            assertEquals(listOf(0L, 60_000L), starts)
            h.data.sync.reconcile(SyncDemand(SyncResource.Notification("notice")))
            assertEquals(60_000L, starts.last())
        } finally { h.close() }
    }

    @Test fun continuingListScopesHonourOverloadBeforeTheNextGetWithoutRestampingTheFailure() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this)
        val starts = mutableListOf<Long>()
        h.data.sync.waitForTargetedBatch = { }
        h.before = { _, _ ->
            starts += testScheduler.currentTime
            if (starts.size == 1) throw ApiProblem("unavailable", 503, "Retry", retryAfterMillis = 60_000)
        }
        try {
            h.seed()
            assertFailsWith<ApiProblem> {
                h.data.sync.reconcile(SyncDemand(SyncResource.List(f.snapshot.lists.single().id), listScopes = setOf("scope-a", "scope-b")))
            }
            assertEquals(listOf(0L), starts, "Overload unwinds the cohort without sleeping inside its lane/permit")
            h.data.sync.reconcile(SyncDemand(SyncResource.List(f.snapshot.lists.single().id), listScopes = setOf("scope-a", "scope-b")))
            assertEquals(listOf(0L, 60_000L, 60_000L, 60_000L), starts)
            h.data.sync.reconcile(SyncDemand(SyncResource.Notification("notice")))
            assertEquals(60_000L, starts.last(), "replay waits outside admission without restamping the failure")
        } finally { h.close() }
    }

    @Test fun retryDeadlineIsCappedAndDoesNotLeakIntoReplacementSessions() = runTest {
        for (replace in listOf(false, true)) {
            val baseline = testScheduler.currentTime
            val cache = memoryCache { f.now }
            var revision = 0L
            val call = GeneratedCall({ error("No HTTP needed") }, { "fixture-session" }, sessionRevision = { revision })
            val starts = mutableListOf<Long>()
            val scheduler = TargetedSyncScheduler(cache, call, backgroundScope, Semaphore(2), { _, _ ->
                starts += testScheduler.currentTime - baseline
                if (starts.size == 1) throw ApiProblem("unavailable", 503, "Retry", retryAfterMillis = 900_000)
                false
            }, {}, { f.now + (testScheduler.currentTime - baseline).milliseconds })
            scheduler.batchMillis = 0
            try {
                cache.initialize()
                assertFailsWith<ApiProblem> { scheduler.request(listOf(SyncDemand(SyncResource.Today))) }
                if (replace) revision++
                scheduler.request(listOf(SyncDemand(SyncResource.NotificationPrefs)))
                assertEquals(listOf(0L, if (replace) 0L else 300_000L), starts)
            } finally { cache.close() }
        }
    }

    @Test fun failedApplicationsGrowReconnectBackoffAndOnlyAcknowledgementResetsIt() = runTest {
        for (failure in listOf("apply", "ack")) for (concurrent in listOf(false, true)) {
            val baseline = testScheduler.currentTime
            val starts = mutableListOf<Long>()
            var acknowledged = 0L
            val live = LiveSync(StreamSource { _, receive ->
                starts += testScheduler.currentTime - baseline
                receive(StreamFrame.Heartbeat)
                receive(StreamFrame.Tickle(1, "item.changed", "item", "item", false))
            }, { acknowledged }, { if (failure == "apply" && starts.size <= 3) error("application unavailable") }, {
                if (failure == "ack" && starts.size <= 3) error("ack unavailable")
                acknowledged = it
            }, {},
                { f.now + testScheduler.currentTime.milliseconds }, { 0.0 }, { concurrent })
            val job = backgroundScope.launch { live.run() }
            try {
                runCurrent(); advanceTimeBy(7_000); runCurrent()
                assertEquals(listOf(0L, 1_000L, 3_000L, 7_000L), starts, "failed applications must not reset backoff ($concurrent)")
                assertEquals(1L, acknowledged)
                advanceTimeBy(1_000); runCurrent()
                assertEquals(8_000L, starts.last(), "successful application and ack reset backoff")
            } finally { job.cancelAndJoin() }
        }
    }
}
