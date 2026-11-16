package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientSyncPrefixTest {
    @Test fun deliveredFramesAreAdmittedWhileEarlierReadIsWaitingAndAckOnlyAppliedPrefix() = runTest {
        val first = CompletableDeferred<Unit>()
        val second = CompletableDeferred<Unit>()
        val admitted = mutableListOf<Long>()
        val acknowledgements = mutableListOf<Long>()
        var cursor = 0L
        val source = StreamSource { _, receive ->
            receive(frame(4)); receive(frame(9))
            // A finite recorded stream waits until its delivered reads settle.
        }
        val live = LiveSync(source, { cursor }, { frame ->
            admitted += frame.seq
            if (frame.seq == 4L) first.await() else second.await()
        }, { cursor = it; acknowledgements += it }, {}, { Instant.fromEpochSeconds(0) }, concurrentApply = { true })
        val connection = launch { live.connectOnce() }
        runCurrent()
        assertEquals(listOf(4L, 9L), admitted, "Frame delivery must not block behind an earlier resource read")
        second.complete(Unit); runCurrent()
        assertTrue(acknowledgements.isEmpty(), "A later applied frame must not pass an unapplied prefix")
        first.complete(Unit); connection.join()
        assertEquals(9L, acknowledgements.last())
    }

    @Test fun failedPrefixDoesNotAckUnrelatedSuccessAndReconnectReplaysTheHole() = runTest {
        var cursor = 0L; var fail = true
        val secondInstalled = CompletableDeferred<Unit>()
        val applied = mutableListOf<Long>(); val acknowledgements = mutableListOf<Long>()
        val live = LiveSync(StreamSource { _, receive -> receive(frame(4)); receive(frame(9)) }, { cursor }, { event ->
            if (event.seq == 4L && fail) { secondInstalled.await(); error("fixture failure") }
            applied += event.seq
            if (event.seq == 9L) secondInstalled.complete(Unit)
        }, { cursor = it; acknowledgements += it }, {}, { Instant.fromEpochSeconds(0) }, concurrentApply = { true })
        assertTrue(runCatching { live.connectOnce() }.isFailure)
        assertEquals(listOf(9L), applied)
        assertTrue(acknowledgements.isEmpty())
        fail = false; live.connectOnce()
        assertEquals(listOf(9L, 4L, 9L), applied)
        assertEquals(9L, cursor)
    }

    private fun frame(seq: Long) = StreamFrame.Tickle(seq, "item.changed", "item", "item-$seq", false)
}
