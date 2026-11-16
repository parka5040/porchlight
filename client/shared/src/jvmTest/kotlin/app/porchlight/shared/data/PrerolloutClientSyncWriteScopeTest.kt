package app.porchlight.shared.data

import app.porchlight.shared.api.models.ListMode
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class PrerolloutClientSyncWriteScopeTest {
    private val f = SyncBundleFixtures

    @Test fun identitylessCommittedOccurrenceWriteAwaitsThrottledFullRepair() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        try {
            val result = assertIs<WriteResult.Success<*>>(h.data.occurrences.patch("uncached", 1, ItemEdit().text("status", "done")))
            assertFalse(result.refreshPending, "A committed write with no known parent must await a full repair, not discard its demand")
            assertEquals(listOf("/v1/snapshot", "/v1/board", "/v1/agenda").sorted(), h.getPaths().sorted())
        } finally { h.close() }
    }

    @Test fun listWriteIncludesItsExplicitOccurrenceEvenWhenItWasNotRetained() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed(); h.scopedList = true
        val list = f.snapshot.lists.single().copy(mode = ListMode.per_occurrence, occurrenceId = null)
        h.cache.inSession { h.cache.putList(it, list) }
        try {
            assertTrue(h.data.lists.scopedAfterWrite!!(list.id, f.snapshot.occurrences.single().id))
            assertEquals(setOf(null, f.snapshot.occurrences.single().id),
                h.requests.map { it.second.parameters["occurrence_id"] }.toSet(),
                "List write coverage must include the changed scope as well as the template")
        } finally { h.close() }
    }

    @Test fun capableNotFoundBundleEvictsAndReconcilesInboxWithoutSnapshotOrBinaryFetch() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed()
        val item = f.snapshot.items.single().id
        h.before = { _, url -> if (url.encodedPath.endsWith("/sync")) throw ApiProblem("not-found", 404, "Not found") }
        try {
            h.data.applyStreamFrame(StreamFrame.Tickle(100, "item.changed", "item", item, false))
            assertTrue(h.cache.view.value.items.isEmpty()); assertTrue(h.cache.view.value.attachments.isEmpty())
            assertEquals(listOf("/v1/items/$item/sync", "/v1/notifications").sorted(), h.getPaths().sorted())
        } finally { h.close() }
    }

    @Test fun capabilityWithoutDifferentialVerificationKeepsLegacyReads() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this); h.seed(); h.data.targetedSyncVerified = false
        try {
            h.data.applyStreamFrame(StreamFrame.Tickle(100, "item.changed", "item", f.snapshot.items.single().id, false))
            assertEquals(listOf("/v1/snapshot", "/v1/board", "/v1/agenda").sorted(), h.getPaths().sorted())
        } finally { h.close() }
    }
}
