package app.porchlight.shared.data

import app.porchlight.shared.screens.planning.formatPlanningTime
import java.util.Locale
import kotlin.test.*
import kotlin.time.Instant

class PlanningPresentationTest {
    @Test fun memberZoneAndLocaleAcrossDstAndDateBoundary() {
        val old = Locale.getDefault()
        try {
            Locale.setDefault(Locale.US)
            val instant = Instant.parse("2026-10-06T00:15:00Z")
            val us = formatPlanningTime(instant, "America/Los_Angeles")
            assertTrue(us.contains("Oct 5"), us)
            assertTrue(us.replace('\u202f', ' ').contains("5:15 PM"), us)
            Locale.setDefault(Locale.GERMANY)
            val german = formatPlanningTime(instant, "Europe/Berlin")
            assertTrue(german.contains("02:15"), german)
            assertNotEquals(us, german)
            val repeated1 = formatPlanningTime(Instant.parse("2026-11-01T08:30:00Z"), "America/Los_Angeles")
            val repeated2 = formatPlanningTime(Instant.parse("2026-11-01T09:30:00Z"), "America/Los_Angeles")
            assertNotEquals(repeated1, repeated2, "DST repeated local times need offset/zone distinction")
        } finally { Locale.setDefault(old) }
    }
}
