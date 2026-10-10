package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class CacheBehaviorTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun snapshotDifferenceEvictsRelatedDataAndKeepsPending() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("visible", true)), occurrences = listOf(occurrenceRead("occ", "visible"))))
            cache.replaceBoard(listOf(card("occ", "A").copy(id = "visible")))
            assertTrue(cache.view.value.items.single().pendingDerivation)
            cache.install(emptySnapshot(2))
            assertTrue(cache.view.value.items.isEmpty()); assertTrue(cache.view.value.occurrences.isEmpty()); assertTrue(cache.view.value.board.isEmpty())
        } finally { cache.close() }
    }
    @Test fun failedMidLoadNeverPromotesAndCleanupUsesEndAndKeepsUndated() = runTest {
        val cache = memoryCache { now }
        try {
            val old = occurrenceRead("old", "one").copy(dueAt = now - 181.days)
            val boundary = occurrenceRead("boundary", "one").copy(dueAt = now - 180.days)
            val span = occurrenceRead("span", "one").copy(startsAt = now - 181.days, endsAt = now + 1.days)
            val undated = occurrenceRead("undated", "one")
            cache.install(emptySnapshot(3).copy(items = listOf(itemRead("one")), occurrences = listOf(old, boundary, span, undated)))
            assertEquals(setOf("boundary", "span", "undated"), cache.view.value.occurrences.map { it.id }.toSet())
            val prior = cache.view.value
            var loaded = 0
            assertFailsWith<IllegalStateException> {
                cache.installStaged(emptySnapshot(4).copy(items = listOf(itemRead("new")), occurrences = listOf(undated))) {
                    if (++loaded == 2) error("failure after two staging rows")
                }
            }
            assertEquals(prior, cache.view.value)
            cache.clear(); assertNull(cache.view.value.sync); assertTrue(cache.view.value.items.isEmpty())
        } finally { cache.close() }
    }
}
