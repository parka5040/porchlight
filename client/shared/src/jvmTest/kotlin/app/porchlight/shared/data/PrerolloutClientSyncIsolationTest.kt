package app.porchlight.shared.data

import app.porchlight.shared.live.StreamFrame
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncIsolationTest {
    private val f = SyncBundleFixtures

    @Test fun blockedContentLaneDoesNotConsumeTheIndependentInboxSlot() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        val item = f.snapshot.items.single().id
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val inboxStarted = CompletableDeferred<Unit>()
        val admissions = Channel<Unit>(Channel.UNLIMITED)
        h.data.sync.targetedDemandAdmitted = { admissions.trySend(Unit) }
        h.before = { _, url -> when {
            url.encodedPath.endsWith("/sync") -> { started.complete(Unit); release.await() }
            url.encodedPath == "/v1/notifications" -> inboxStarted.complete(Unit)
        } }
        try {
            val bundle = async { h.data.applyStreamFrame(StreamFrame.Tickle(100, "item.changed", "item", item, false)) }
            admissions.receive(); started.await()
            val comment = async { h.data.applyStreamFrame(StreamFrame.Tickle(101, "comment.changed", "comment", f.snapshot.comments.single().id, false, item)) }
            admissions.receive()
            val inbox = async { h.data.applyStreamFrame(StreamFrame.Tickle(102, "notification.invalidated", "notification", "notice", false)) }
            admissions.receive()
            // Cache IO is intentionally real; a virtual timeout can fire before its dispatcher gets a turn.
            assertNotNull(withContext(Dispatchers.Default) { withTimeoutOrNull(5_000) { inboxStarted.await() } },
                "An independent inbox read must start while content is blocked")
            inbox.await()
            assertFalse(bundle.isCompleted); assertFalse(comment.isCompleted)
            release.complete(Unit); bundle.await(); comment.await()
        } finally { release.complete(Unit); h.close() }
    }

    @Test fun legacyDeletedRepresentationStillEvictsWithoutWaitingForSnapshot() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this, targeted = false); h.seed()
        val item = f.snapshot.items.single()
        h.itemResponse = h.itemResponse.copy(deletedAt = f.now)
        try {
            assertIs<WriteResult.Success<*>>(h.data.items.edit(item.id, item.version, ItemEdit().text("title", "Removed task")))
            assertTrue(h.getPaths().isEmpty(), "Legacy deletion receipts already evict projections without waiting for an active snapshot")
            assertTrue(h.cache.view.value.items.isEmpty())
            assertTrue(h.cache.view.value.occurrences.isEmpty())
            assertTrue(h.cache.view.value.board.isEmpty())
        } finally { h.close() }
    }
}
