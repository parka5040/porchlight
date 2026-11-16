package app.porchlight.shared.data

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

/** The current wire protocol has no replay-completion barrier. Ready/heartbeat cannot safely close a worker stream. */
@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncCatchUpProtocolTest {
    private val f = SyncBundleFixtures

    @Test fun readyIsOpeningNotReplayCompletionAndNeverAcknowledgesItsCursor() = runTest {
        val replay = CompletableDeferred<Unit>()
        val commit = CompletableDeferred<Unit>()
        val admitted = CompletableDeferred<Unit>()
        var cursor = 7L
        val frames = mutableListOf<StreamFrame>()
        val parser = SseParser()
        val ready = listOf("event: ready", "data: {\"cursor\":7}", "").mapNotNull(parser::line).single()
        val source = StreamSource { after, receive ->
            assertEquals(7L, after)
            receive(ready)
            // Server sends ready before its first replay scan, even with work already outstanding.
            replay.await()
            receive(StreamFrame.Tickle(19, "item.invalidated", "item", "visible-item", false))
        }
        val live = LiveSync(source, { cursor }, { frame -> frames += frame; admitted.complete(Unit); commit.await() },
            { cursor = it }, { error("A valid cursor must not resnapshot") }, { f.now }, concurrentApply = { true })
        val catchUp = async { live.connectOnce() }
        runCurrent()
        assertFalse(catchUp.isCompleted)
        assertEquals(7L, cursor, "Ready is not a tickle acknowledgement or an opening high-watermark")
        replay.complete(Unit); admitted.await()
        assertFalse(catchUp.isCompleted, "EOF must await the targeted commit")
        assertEquals(7L, cursor)
        commit.complete(Unit); catchUp.await()
        assertEquals(listOf(19L), frames.filterIsInstance<StreamFrame.Tickle>().map { it.seq })
        assertEquals(19L, cursor)
    }

    @Test fun heartbeatDuringReplayIsNotProofOfExhaustionAndNumericGapsAreLegal() = runTest {
        var cursor = 7L
        val applied = mutableListOf<Long>()
        val source = StreamSource { _, receive ->
            receive(StreamFrame.Ready)
            receive(StreamFrame.Heartbeat) // The server may heartbeat before/within a delayed replay read.
            receive(StreamFrame.Tickle(31, "notification.invalidated", "notification", "private-notice", false))
            receive(StreamFrame.Heartbeat)
            receive(StreamFrame.Tickle(87, "settings.changed", "availability", "member", false))
        }
        val live = LiveSync(source, { cursor }, { applied += it.seq }, { cursor = it },
            { error("Gaps must not force a snapshot") }, { f.now }, concurrentApply = { true })
        live.connectOnce()
        assertEquals(listOf(31L, 87L), applied)
        assertEquals(87L, cursor)
    }
}
