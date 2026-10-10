package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class RebaseCacheTest {
    @Test fun planningAndCollaborationShareAtomicSnapshotEvictionAndClear() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val availability = AvailabilityResponse(collabId("self"), 1, emptyList(), emptyList())
        val suggestion = WireJson.decodeFromString<SuggestionResponse>("""{"id":"${collabId("one")}","member_id":"${collabId("self")}","item_id":"${collabId("item")}","occurrence_id":"${collabId("occurrence")}","proposed_start":"2026-10-07T09:00:00Z","proposed_end":"2026-10-07T10:00:00Z","status":"proposed","run_id":"${collabId("two")}","version":3,"occurrence_version":9,"created_at":"2026-10-06T09:00:00Z","conflicts":[]}""")
        val snapshot = emptySnapshot(4).copy(
            availability = listOf(availability), suggestions = listOf(suggestion),
            lists = listOf(listFixture().copy(itemId = collabId("item"))),
            comments = listOf(commentFixture()), attachments = listOf(attachmentFixture()),
        )
        try {
            cache.install(snapshot)
            val committed = cache.view.value
            assertEquals(snapshot.availability, committed.availability)
            assertEquals(snapshot.suggestions, committed.suggestions)
            assertEquals(snapshot.lists, committed.lists)
            assertEquals(snapshot.comments, committed.comments)
            assertEquals(snapshot.attachments, committed.attachments)
            assertFailsWith<IllegalStateException> {
                cache.installStaged(emptySnapshot(5), beforeCommit = { error("rollback both sides") })
            }
            assertEquals(committed, cache.view.value)
            cache.evictItem(collabId("item"))
            assertTrue(cache.view.value.suggestions.isEmpty())
            assertTrue(cache.view.value.lists.isEmpty())
            assertTrue(cache.view.value.comments.isEmpty())
            assertTrue(cache.view.value.attachments.isEmpty())
            assertEquals(listOf(availability), cache.view.value.availability)
            cache.clear()
            assertEquals(CacheView(), cache.view.value)
        } finally { cache.close() }
    }
}
