package app.porchlight.shared.data

import app.porchlight.shared.screens.planning.formatPlanningTime
import java.util.Locale
import kotlin.test.*
import kotlin.time.Instant

class PlanningPresentationTest {
    @Test fun memberZoneAndLocaleAcrossDstAndDateBoundary() {
        val instant = Instant.parse("2026-10-06T00:15:00Z")
        val us = formatPlanningTime(instant, "America/Los_Angeles", Locale.US)
        assertTrue(us.contains("Oct 5"), us)
        assertTrue(us.replace(' ', ' ').contains("5:15 PM"), us)
        val german = formatPlanningTime(instant, "Europe/Berlin", Locale.GERMANY)
        assertTrue(german.contains("02:15"), german)
        assertNotEquals(us, german)
        val repeated1 = formatPlanningTime(Instant.parse("2026-11-01T08:30:00Z"), "America/Los_Angeles", Locale.US)
        val repeated2 = formatPlanningTime(Instant.parse("2026-11-01T09:30:00Z"), "America/Los_Angeles", Locale.US)
        assertNotEquals(repeated1, repeated2, "DST repeated local times need offset/zone distinction")
    }
}
