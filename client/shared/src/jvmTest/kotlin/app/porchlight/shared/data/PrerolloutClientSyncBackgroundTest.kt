package app.porchlight.shared.data

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncBackgroundTest {
    private val f = SyncBundleFixtures
    private fun live(h: PrerolloutClientSyncDispatchTest.Fixture, source: StreamSource) =
        LiveSync(source, { h.cache.sync.value?.cursor }, { h.data.applyStreamFrame(it) },
            { h.cache.acknowledgeCursor(it) }, { h.data.sync.resnapshot() }, { f.now }, concurrentApply = { true })

    @Test fun unchangedCursorWorkerRunIssuesZeroSnapshotBoardOrAgendaRequests() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        val cursor = h.cache.sync.value?.cursor
        var closed = false
        val stream = live(h, StreamSource { after, receive ->
            assertEquals(cursor, after)
            try { receive(StreamFrame.Ready); receive(StreamFrame.CaughtUp(after)) } finally { closed = true }
        })
        try {
            assertTrue(backgroundSafetyNet(false, false, h.data.targetedSync, cursor, stream,
                { h.data.notifications.refresh() }, { h.data.sync.refresh() }))
            assertTrue(h.requests.isEmpty(), "Unchanged capable worker must not fetch snapshots, board, agenda or inbox")
            assertEquals(cursor, h.cache.sync.value?.cursor); assertTrue(closed)
        } finally { h.close() }
    }
    @Test fun backlogUsesRealTargetedReplacementBeforeAcknowledgementAndClosesAtBarrier() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        val id = f.snapshot.items.single().id
        val before = h.cache.sync.value?.cursor
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        h.before = { _, url -> if (url.encodedPath.endsWith("/sync")) { started.complete(Unit); release.await() } }
        val stream = live(h, StreamSource { after, receive ->
            receive(StreamFrame.Ready)
            receive(StreamFrame.Tickle(after + 10, "item.invalidated", "item", id, false))
            receive(StreamFrame.CaughtUp(after + 9))
            error("Must close after barrier")
        })
        try {
            // Cache commits use real IO; do not let a virtual 60-second timeout outrun those dispatches.
            val worker = async(Dispatchers.Default) { backgroundSafetyNet(false, false, true, before, stream, {}, { h.data.sync.refresh() }) }
            started.await(); assertEquals(before, h.cache.sync.value?.cursor); assertFalse(worker.isCompleted)
            release.complete(Unit); assertTrue(worker.await())
            assertEquals(listOf("/v1/items/$id/sync"), h.getPaths())
            assertEquals(checkNotNull(before) + 10, h.cache.sync.value?.cursor)
        } finally { release.complete(Unit); h.close() }
    }
    @Test fun resnapshotUsesTheExistingThrottledFullRefreshAndItsSnapshotCursor() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        h.snapshotCursor = 400
        val stream = live(h, StreamSource { _, receive -> receive(StreamFrame.Resnapshot) })
        val waits = mutableListOf<Long>()
        h.data.sync.waitForReadRetry = { waits += it; delay(it) }
        try {
            repeat(2) {
                assertTrue(withContext(Dispatchers.Default) {
                    backgroundCatchUp(true, h.cache.sync.value?.cursor, stream) { error("Wrong fallback") }
                })
            }
            assertEquals(2, h.getPaths().count { it == "/v1/snapshot" })
            assertTrue(waits.any { it >= 30_000 }, "Terminal resets must retain the shared snapshot-start floor")
            assertEquals(400, h.cache.sync.value?.cursor)
        } finally { h.close() }
    }
    @Test fun legacyServerStillPerformsTheExistingFullSafetyNet() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this, targeted = false); h.seed()
        try {
            assertTrue(withContext(Dispatchers.Default) {
                backgroundSafetyNet(false, false, h.data.targetedSync, h.cache.sync.value?.cursor, null,
                    { error("Not a healthy foreground stream") }, { h.data.sync.refresh() })
            })
            assertTrue(h.getPaths().containsAll(listOf("/v1/snapshot", "/v1/board", "/v1/agenda")))
        } finally { h.close() }
    }
    @Test fun workerPassLeaseRejectsAcknowledgementAfterIdentityChangesBetweenApplyAndAck() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        var epoch = 0L; h.cache.sessionEpoch = { epoch }
        val applied = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val before = checkNotNull(h.cache.sync.value?.cursor)
        val stream = LiveSync(StreamSource { _, receive ->
            receive(StreamFrame.Tickle(before + 10, "item.invalidated", "item", f.snapshot.items.single().id, false))
            receive(StreamFrame.CaughtUp(before + 10))
        }, { h.cache.sync.value?.cursor }, {
            h.data.applyStreamFrame(it); applied.complete(Unit); release.await()
        }, { seq -> h.cache.inSession { h.cache.acknowledgeCursor(seq) } }, {}, { f.now }, concurrentApply = { true })
        try {
            val worker = async(Dispatchers.Default) {
                // Same whole-pass cache lease as HouseholdSyncWorker; refetch-only leases leave an ack gap.
                h.cache.inSession { backgroundCatchUp(true, before, stream) { error("No fallback") } }
            }
            applied.await(); epoch++
            h.cache.clear(); h.cache.install(f.snapshot.copy(cursor = 3))
            assertEquals(3, h.cache.sync.value?.cursor)
            release.complete(Unit)
            assertFailsWith<CancellationException> { worker.await() }
            assertEquals(3, h.cache.sync.value?.cursor, "Old applied work must not acknowledge into the new identity")
        } finally { release.complete(Unit); h.close() }
    }
    @Test fun healthyStayConnectedStreamRetainsItsUnreadOnlySafetyNet() = runTest {
        var unread = 0
        assertTrue(backgroundSafetyNet(true, true, true, 41, null, { unread++ }, { error("No full read") }))
        assertEquals(1, unread)
    }
}
