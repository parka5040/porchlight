package app.porchlight.shared.data

import app.porchlight.shared.data.syncmodel.HouseholdModel
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientSyncModelContractTest {
    @Test fun timedSpanOverlappingWindowIsIncluded() {
        val m = HouseholdModel()
        val o = m.occurrences.first().copy(startsAt = HouseholdModel.from - kotlin.time.Duration.parse("1h"), endsAt = HouseholdModel.from + kotlin.time.Duration.parse("1h"))
        m.occurrences = listOf(o)
        assertEquals(listOf(o.id), m.snapshot(m.readers.first()).occurrences.map { it.id })
    }

    @Test fun gapResolvesToFirstValidMinuteNotShiftedMinute() {
        val m = HouseholdModel()
        m.items = listOf(m.items[2])
        m.occurrences = listOf(m.occurrences.first { it.itemId == m.items.single().id }.copy(localDue = "2027-03-14T02:30:00", startsAt = null, dueAt = null))
        val w = ItemAgendaWindow(m.readers.first(), Instant.parse("2027-03-14T07:00:00Z"), Instant.parse("2027-03-14T07:01:00Z"))
        assertEquals(1, m.agenda(m.readers.first(), w).size)
    }

    @Test fun agendaUsesRequestedMemberZoneRatherThanHouseholdZone() {
        val m = HouseholdModel()
        val reader = m.readers.first()
        m.members = m.members.map { if (it.id == reader) it.copy(tzid = "Asia/Tokyo") else it }
        val item = m.items[2]
        m.items = listOf(item)
        m.occurrences = listOf(m.occurrences.first { it.itemId == item.id }.copy(localDue = "2026-10-06T09:00:00", startsAt = null, dueAt = null))
        assertEquals(1, m.agenda(reader, ItemAgendaWindow(reader, Instant.parse("2026-10-06T00:00:00Z"), Instant.parse("2026-10-06T00:01:00Z"))).size)
    }

    @Test fun remindersRequireRecipientOpenStatusAndFireWindow() {
        val m = HouseholdModel()
        val reader = m.readers.first()
        m.items = m.items.map { it.copy(ownerId = m.readers[1], assignees = listOf(m.readers[1])) }
        assertTrue(m.snapshot(reader).scheduledReminders.isEmpty(), "Unassigned reader is not a reminder recipient")
    }
}
