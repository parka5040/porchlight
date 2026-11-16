package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncCatchUpTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun tick(seq: Long) = StreamFrame.Tickle(seq, "item.changed", "item", "item", false)
    private fun control(cursor: String, id: String? = null): StreamFrame? {
        val parser = SseParser()
        id?.let { parser.line("id: $it") }
        parser.line("event: caught_up"); parser.line("data: {\"cursor\":$cursor}")
        return parser.line("")
    }
    @Test fun negotiatedCaughtUpIsAControlNotATickle() {
        assertEquals(StreamFrame.CaughtUp(42), control("42"))
        assertFailsWith<InvalidStream> { control("-1") }
        assertFailsWith<InvalidStream> { control("null") }
        assertFailsWith<InvalidStream> { control("42", "42") }
    }
    @Test fun barrierWaitsForAllEarlierApplicationsAndCursorCommitIncludingNewerThanWatermark() = runTest {
        var cursor = 1L
        val release = CompletableDeferred<Unit>(); val commit = CompletableDeferred<Unit>()
        val applying = Channel<Long>(Channel.UNLIMITED); val applied = mutableListOf<Long>()
        var closed = false; var afterBarrier = false
        val live = LiveSync(StreamSource { after, receive ->
            assertEquals(1, after)
            try {
                receive(StreamFrame.Ready); receive(tick(4)); receive(tick(19))
                receive(StreamFrame.CaughtUp(9))
                afterBarrier = true; receive(tick(21))
            } finally { closed = true }
        }, { cursor }, { applying.send(it.seq); release.await(); applied += it.seq },
            { commit.await(); cursor = it }, {}, { now }, concurrentApply = { true })
        val run = async { live.catchUp() }
        assertEquals(4, applying.receive()); assertEquals(19, applying.receive())
        runCurrent(); assertFalse(run.isCompleted); assertEquals(1, cursor)
        release.complete(Unit); runCurrent()
        assertFalse(run.isCompleted, "Applying a frame is not the same as committing its acknowledgement")
        commit.complete(Unit)
        assertTrue(run.await()); assertEquals(listOf(4L, 19L), applied)
        assertEquals(19, cursor, "caught_up's lower watermark must not regress or replace the tickle cursor")
        assertTrue(closed); assertFalse(afterBarrier)
    }
    @Test fun filteredOnlyBacklogFinishesWithoutAcknowledgingTheControlWatermark() = runTest {
        var cursor = 7L
        val live = LiveSync(StreamSource { after, receive ->
            assertEquals(7, after); receive(StreamFrame.Ready); receive(StreamFrame.CaughtUp(900))
        }, { cursor }, { error("No visible events") }, { cursor = it }, {}, { now })
        assertTrue(live.catchUp()); assertEquals(7, cursor)
        assertFalse(live.hasRecentStreamFrame(), "A closed catch-up socket is not a healthy stay-connected stream")
    }
    @Test fun readyHeartbeatAndEofCannotProveCatchUp() = runTest {
        for (hang in listOf(false, true)) {
            val live = LiveSync(StreamSource { _, receive ->
                receive(StreamFrame.Ready); receive(StreamFrame.Heartbeat)
                if (hang) { delay(60_001); receive(StreamFrame.CaughtUp(42)) }
            }, { 1 }, {}, { error("No tickle") }, {}, { now })
            assertFalse(live.catchUp())
        }
    }
    @Test fun timeoutClosesTheStreamAndLeavesTheUnappliedPrefixUnacknowledged() = runTest {
        var cursor = 1L; var closed = false
        val live = LiveSync(StreamSource { _, receive ->
            try { receive(tick(4)); receive(StreamFrame.CaughtUp(4)) } finally { closed = true }
        }, { cursor }, { awaitCancellation() }, { cursor = it }, {}, { now }, concurrentApply = { true })
        assertFalse(live.catchUp()); assertEquals(1, cursor); assertTrue(closed)
    }
    @Test fun timeoutRetainsAppliedPrefixAndNextRunReplaysTheUnacknowledgedRemainder() = runTest {
        var cursor = 1L; var firstRun = true
        val openings = mutableListOf<Long>(); val committed = mutableListOf<Long>()
        val live = LiveSync(StreamSource { after, receive ->
            openings += after; receive(tick(4)); receive(tick(9)); receive(StreamFrame.CaughtUp(9))
        }, { cursor }, {
            if (it.seq == 9L && firstRun) awaitCancellation()
            committed += it.seq
        }, { cursor = it }, {}, { now }, concurrentApply = { true })
        assertFalse(live.catchUp()); assertEquals(4, cursor)
        firstRun = false
        assertTrue(live.catchUp()); assertEquals(9, cursor)
        assertEquals(listOf(1L, 4L), openings); assertEquals(listOf(4L, 9L), committed)
    }
    @Test fun failedApplicationCannotTurnTheBarrierIntoSuccess() = runTest {
        var cursor = 1L
        val live = LiveSync(StreamSource { _, receive -> receive(tick(4)); receive(StreamFrame.CaughtUp(4)) },
            { cursor }, { throw IllegalStateException("Rejected read") }, { cursor = it }, {}, { now }, concurrentApply = { true })
        assertFailsWith<IllegalStateException> { live.catchUp() }; assertEquals(1, cursor)
    }
    @Test fun terminalResetUsesFullRefreshButDrainDoesNotClaimCompletion() = runTest {
        for (terminal in listOf(StreamFrame.Resnapshot, StreamFrame.ProjectionChanged(), StreamFrame.Drain)) {
            var snapshots = 0
            val live = LiveSync(StreamSource { _, receive -> receive(terminal) }, { 4 }, {}, {}, { snapshots++ }, { now })
            assertEquals(terminal != StreamFrame.Drain, live.catchUp())
            assertEquals(if (terminal == StreamFrame.Drain) 0 else 1, snapshots)
        }
    }
    @Test fun legacyAndMissingOrInvalidLocalCursorUseTodaysFullReadWithoutOpeningAStream() = runTest {
        val live = LiveSync(StreamSource { _, _ -> error("No stream on fallback") }, { 4 }, {}, {}, {}, { now })
        var reads = 0
        for ((capable, cursor) in listOf(false to 4L, true to null, true to -1L))
            assertTrue(backgroundCatchUp(capable, cursor, live) { reads++ })
        assertEquals(3, reads)
    }
    @Test fun externalCancellationIsNotConvertedIntoWorkerRetry() = runTest {
        val live = LiveSync(StreamSource { _, _ -> awaitCancellation() }, { 1 }, {}, {}, {}, { now })
        assertFailsWith<TimeoutCancellationException> { withTimeout(10) { live.catchUp() } }
    }
    @Test fun longLivedStreamKeepsApplyingAfterCaughtUp() = runTest {
        var cursor = 1L
        val applied = mutableListOf<Long>()
        val live = LiveSync(StreamSource { _, receive ->
            receive(StreamFrame.Ready); receive(tick(4)); receive(StreamFrame.CaughtUp(9)); receive(tick(21))
        }, { cursor }, { applied += it.seq }, { cursor = it }, {}, { now }, concurrentApply = { true })
        live.connectOnce(); assertEquals(listOf(4L, 21L), applied); assertEquals(21, cursor)
    }
    @Test fun desktopResumeUsesLongLivedReplayBeforeContinuingWithNewPublications() = runTest {
        val connections = Channel<Long>(Channel.UNLIMITED)
        var signal: (Boolean) -> Unit = {}
        val bus = object : DesktopSleepBus {
            override fun watch(receive: (Boolean) -> Unit) { signal = receive }
            override fun close() {}
        }
        var cursor = 1L; var opens = 0
        val live = LiveSync(StreamSource { after, receive ->
            connections.send(after); receive(StreamFrame.Ready)
            if (++opens == 2) { receive(tick(4)); receive(StreamFrame.CaughtUp(9)); receive(tick(21)) }
            awaitCancellation()
        }, { cursor }, {}, { cursor = it }, { error("Resume must not snapshot") }, { now }, concurrentApply = { true })
        val monitor = DesktopResumeMonitor(live::networkChanged, backgroundScope) { bus }
        val foreground = backgroundScope.launch { live.run() }
        try {
            assertEquals(1, connections.receive())
            signal(true); runCurrent(); assertEquals(1, opens)
            signal(false); assertEquals(1, connections.receive()); runCurrent()
            assertEquals(21, cursor)
        } finally { monitor.close(); foreground.cancelAndJoin() }
    }
    @Test fun boundedReplayYieldsTheStaleLongLivedSocketAndNeverCreatesParallelPrefixTrackers() = runTest {
        val connections = Channel<Int>(Channel.UNLIMITED)
        var opens = 0; var active = 0; var maximumActive = 0; var cursor = 1L
        val live = LiveSync(StreamSource { _, receive ->
            val connection = ++opens; active++; maximumActive = maxOf(maximumActive, active)
            try {
                connections.send(connection); receive(StreamFrame.Ready)
                if (connection == 2) { receive(tick(4)); receive(StreamFrame.CaughtUp(4)) }
                awaitCancellation()
            } finally { active-- }
        }, { cursor }, {}, { cursor = it }, {}, { now }, concurrentApply = { true })
        val foreground = backgroundScope.launch { live.run() }
        assertEquals(1, connections.receive())
        val catchUp = async { live.catchUp() }
        assertEquals(2, connections.receive()); assertTrue(catchUp.await())
        assertEquals(3, connections.receive()); assertEquals(4, cursor); assertEquals(1, maximumActive)
        foreground.cancelAndJoin()
    }
}
