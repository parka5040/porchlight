package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Duration.Companion.days

class PrerolloutClientSyncFixACoverageTest {
    private val f = SyncBundleFixtures

    @Test fun occurrenceOmissionDoesNotDeleteUnrequestedAgendaRemindersInboxOrListChecks() = runTest {
        val h = PrerolloutClientSyncDispatchTest().Fixture(this)
        val cache = h.cache
        val occurrence = f.snapshot.occurrences.single()
        val member = f.snapshot.memberSettings.memberId
        val agenda = f.agenda.copy(agenda = f.agenda.agenda!!.copy(dueAt = f.to + 1.days, startsAt = null, endsAt = null))
        val reminder = f.snapshot.scheduledReminders.single().copy(fireAt = f.to + 1.days)
        val template = f.recording<ListResponse>("list-occurrence-template").copy(itemId = occurrence.itemId)
        val checks = f.recording<ListResponse>("list-occurrence-check").copy(itemId = occurrence.itemId, occurrenceId = occurrence.id)
        try {
            cache.install(f.snapshot.copy(scheduledReminders = listOf(reminder), lists = listOf(template)),
                projections = Projections(listOf(f.board), AgendaWindow(f.to, f.to + 2.days, member, listOf(agenda))))
            cache.putList(checks)
            val before = cache.view.value
            // Nine disjoint retained windows require two requests. Only the failed second group
            // covers the cached agenda; the first group still removes the covered occurrence row.
            val windows = (0..8).map { ItemAgendaWindow(member, f.to - (15 - it * 2).days, f.to - (14 - it * 2).days) }
            val page = f.bundle(f.snapshot.copy(occurrences = emptyList(), scheduledReminders = emptyList()), agenda = emptyList(), collaboration = false)
                .let { it.copy(coverage = it.coverage.copy(agenda = windows.take(8).map { window -> AgendaCoverage(member, window.from, window.to, true) }),
                    agenda = windows.take(8).map { window -> SyncAgenda(member, window.from, window.to, emptyList()) }) }
            h.bundleResponse = page
            var requests = 0
            h.before = { _, url -> if (url.encodedPath.endsWith("/sync") && ++requests == 2) error("second window group unavailable") }
            cache.inSession { fence ->
                assertFailsWith<IllegalStateException> { h.data.readTargeted(fence, SyncDemand(SyncResource.Item(occurrence.itemId), agenda = windows.toSet())) }
            }
            assertEquals(2, requests)
            assertEquals(before.sync?.cursor, cache.sync.value?.cursor)
            assertTrue(cache.view.value.occurrences.none { it.id == occurrence.id })
            assertEquals(before.agenda, cache.view.value.agenda, "unrequested retained window")
            assertEquals(before.reminders, cache.view.value.reminders, "outside reminder coverage")
            assertEquals(before.notifications, cache.view.value.notifications, "no inbox coverage")
            assertEquals(before.lists, cache.view.value.lists, "retained occurrence checks")
            cache.evictOccurrence(occurrence.id)
            assertTrue(cache.view.value.agenda.isEmpty(), "explicit deletion still cascades")
            assertTrue(cache.view.value.reminders.isEmpty())
        } finally { h.close() }
    }
}
