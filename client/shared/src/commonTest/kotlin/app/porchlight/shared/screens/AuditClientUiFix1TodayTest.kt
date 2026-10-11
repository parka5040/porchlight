package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.today.*
import kotlinx.datetime.TimeZone
import kotlin.test.*
import kotlin.time.Instant

class AuditClientUiFix1TodayTest {
    @Test fun floatingGapPreservesWallDurationIncludingOverrides() {
        for (detached in listOf(false, true)) {
            val start = "2026-03-08T02:15:00"; val end = "2026-03-08T02:45:00"
            val occurrence = occurrenceRead("occ", "trip").copy(dueDate = null, dueAt = null,
                startsAt = null, endsAt = null, localStart = if (detached) "2026-03-07T02:15:00" else start,
                localEnd = if (detached) "2026-03-07T02:45:00" else end,
                overrideLocalStart = if (detached) start else null, overrideLocalEnd = if (detached) end else null)
            val row = itemRead("trip").copy(kind = "appointment", timeMode = "floating", ownerId = "self", occurrence = occurrence)
            fun groups(now: String) = todaySections(listOf(row), defaultTodaySettings, "self", TimeZone.of("America/New_York"), Instant.parse(now)).associate { it.kind to it.rows }
            assertEquals(listOf(row), groups("2026-03-08T07:10:00Z")[TodaySection.today], "floating gap appointment still running")
            assertEquals(emptyList(), groups("2026-03-08T07:29:59Z")[TodaySection.overdue])
            assertEquals(listOf(row), groups("2026-03-08T07:30:00Z")[TodaySection.overdue])
            assertEquals(emptyList(), groups("2026-03-08T07:30:00Z")[TodaySection.today])
        }
    }
}
