package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.today.*
import kotlinx.datetime.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientUiTodayTest {
    private fun appointment(start: String, end: String, allDay: Boolean = false) = itemRead("trip").copy(
        kind = "appointment", allDay = allDay, ownerId = "self", occurrence = occurrenceRead("occ", "trip").copy(
            dueDate = null, dueAt = null, startsAt = Instant.parse(start), endsAt = Instant.parse(end)))
    private fun groups(row: ItemRead, now: String, zone: String = "UTC") = todaySections(listOf(row),
        defaultTodaySettings, "self", TimeZone.of(zone), Instant.parse(now)).associate { it.kind to it.rows }
    @Test fun multiDayAllDayUsesExclusiveEnd() {
        val row = appointment("2026-10-10T00:00:00Z", "2026-10-13T00:00:00Z", true)
        val during = groups(row, "2026-10-11T12:00:00Z")
        assertEquals(listOf(row), during[TodaySection.today], "ongoing appointment belongs in Today")
        assertTrue(during.getValue(TodaySection.overdue).isEmpty())
        assertTrue(groups(row, "2026-10-13T00:00:00Z").getValue(TodaySection.today).isEmpty())
        assertEquals(listOf(row), groups(row, "2026-10-13T00:00:00Z")[TodaySection.overdue])
    }
    @Test fun overnightAndDstIntervalsOverlapMemberDay() {
        val row = appointment("2026-11-01T03:00:00Z", "2026-11-01T08:00:00Z")
        val during = groups(row, "2026-11-01T06:30:00Z", "America/New_York")
        assertEquals(listOf(row), during[TodaySection.today], "overnight appointment belongs in Today")
        assertTrue(during.getValue(TodaySection.overdue).isEmpty())
        assertEquals(listOf(row), groups(row, "2026-11-01T08:00:00Z", "America/New_York")[TodaySection.overdue])
    }
    @Test fun floatingAllDayAndDetachedBoundsUseLocalEnd() {
        val row = appointment("2026-10-01T00:00:00Z", "2026-10-02T00:00:00Z", true).let { it.copy(timeMode = "floating",
            occurrence = it.occurrence!!.copy(startsAt = null, endsAt = null, localStart = "2026-10-01T00:00:00", localEnd = "2026-10-02T00:00:00",
                overrideLocalStart = "2026-10-10T00:00:00", overrideLocalEnd = "2026-10-13T00:00:00")) }
        assertEquals(listOf(row), groups(row, "2026-10-11T12:00:00Z")[TodaySection.today])
    }
}
