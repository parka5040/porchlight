package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class LiveTest {
    @Test fun parserHandlesCommentsMultilineAndSplitFrames() {
        val p = SseParser()
        assertEquals(StreamFrame.Heartbeat, p.line(":hb"))
        assertNull(p.line("id: 17")); assertNull(p.line("event: occurrence.changed"))
        assertNull(p.line("data: {\"seq\":17,\"kind\":\"occurrence.changed\","))
        assertNull(p.line("data: \"entity_type\":\"occurrence\",\"entity_id\":\"id\"}"))
        assertEquals(StreamFrame.Tickle(17, "occurrence.changed", "occurrence", "id", false), p.line(""))
        assertNull(p.line("event: resnapshot")); assertNull(p.line("data: {\"reason\":\"cursor_expired\"}"))
        assertEquals(StreamFrame.Resnapshot, p.line(""))
    }
    @Test fun parserRejectsCursorMismatchAndBoundsMemory() {
        val p = SseParser()
        p.line("id: 9"); p.line("event: item.changed")
        p.line("data: {\"seq\":10,\"kind\":\"item.changed\",\"entity_type\":\"item\",\"entity_id\":\"id\"}")
        assertFailsWith<InvalidStream> { p.line("") }
        assertFailsWith<InvalidStream> { SseParser().line("data: " + "x".repeat(65537)) }
    }
    @Test fun failedRefetchNeverAcknowledgesTickle() = runTest {
        var cursor = 5L
        val live = LiveSync(source = StreamSource { _, receive -> receive(StreamFrame.Tickle(6,"item.changed","item","id",false)) },
            cursor = { cursor }, apply = { throw IllegalStateException() }, acknowledge = { cursor = it },
            resnapshot = {}, now = { Instant.fromEpochMilliseconds(testScheduler.currentTime) })
        assertFailsWith<IllegalStateException> { live.connectOnce() }
        assertEquals(5, cursor)
    }
    @Test fun snapshotResumesAndDuplicateTicklesAreIgnored() = runTest {
        var cursor = 5L; var reads = 0
        val live = LiveSync(StreamSource { after, receive ->
            assertEquals(5, after)
            receive(StreamFrame.Tickle(5,"item.changed","item","id",false))
            receive(StreamFrame.Tickle(7,"item.changed","item","id",false))
            receive(StreamFrame.Resnapshot)
        }, { cursor }, { reads++ }, { cursor = it }, { cursor = 20 }, { Instant.fromEpochMilliseconds(testScheduler.currentTime) })
        live.connectOnce()
        assertEquals(1, reads); assertEquals(20, cursor)
    }
    @Test fun backoffAndReachabilityUseInjectedClock() {
        assertEquals(1000, reconnectDelay(0, 0.0)); assertEquals(2000, reconnectDelay(1, 0.0))
        assertEquals(60000, reconnectDelay(1000, 0.0)); assertEquals(90000, reconnectDelay(1000, 1.0))
        val r = Reachability(Instant.fromEpochMilliseconds(0))
        assertNull(r.unreachableSince(Instant.fromEpochMilliseconds(299999)))
        assertNotNull(r.unreachableSince(Instant.fromEpochMilliseconds(300000)))
        r.success(Instant.fromEpochMilliseconds(300000))
        assertNull(r.unreachableSince(Instant.fromEpochMilliseconds(300001)))
    }
}
