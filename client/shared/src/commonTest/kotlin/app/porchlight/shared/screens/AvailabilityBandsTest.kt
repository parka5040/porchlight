package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.calendar.*
import kotlinx.datetime.*
import kotlin.test.*

class AvailabilityBandsTest {
    private val member = MemberRead(false, "01999999-0000-7000-8000-000000000001", "01999999-0000-7000-8000-000000000006", "Alex", "user", "#FABD2F", "AX", "America/New_York", "manual", 1)
    private fun bands(date: LocalDate, start: String, end: String, zone: String = member.tzid): List<AvailabilityBand> {
        val zonedMember = member.copy(tzid = zone)
        val rows = listOf(AvailabilityResponse(member.id, 1, listOf(Window(date.dayOfWeek.ordinal, start, end)), emptyList()))
        return availabilityBands(date, TimeZone.of(zone), rows, listOf(zonedMember), setOf(member.id))
    }

    // Server schedule.rs expands endpoints through time.rs resolve_local:
    // gaps snap to the first valid wall minute; folds choose the earlier instant.
    @Test fun springGapStartSnapsToThreeNotThreeThirty() {
        val band = bands(LocalDate(2026, 3, 8), "02:30:45", "03:30:00").single()
        assertEquals(120.0, band.startMinute) // 07:00Z, 03:00 EDT
        assertEquals(30.0, band.durationMinutes)
    }
    @Test fun springGapEndSnapsToThreeNotThreeThirty() {
        val band = bands(LocalDate(2026, 3, 8), "01:30:00", "02:30:00").single()
        assertEquals(90.0, band.startMinute) // 06:30Z
        assertEquals(30.0, band.durationMinutes) // to 07:00Z
        assertTrue(bands(LocalDate(2026, 3, 8), "02:10:00", "02:50:00").isEmpty())
    }
    @Test fun autumnFoldUsesEarlierInstantForBothEndpoints() {
        val date = LocalDate(2026, 11, 1)
        val inside = bands(date, "01:15:00", "01:45:00").single()
        assertEquals(75.0, inside.startMinute) // 05:15Z, not 06:15Z
        assertEquals(30.0, inside.durationMinutes)
        val crossing = bands(date, "01:30:00", "02:30:00").single()
        assertEquals(90.0, crossing.startMinute)
        assertEquals(120.0, crossing.durationMinutes) // 05:30Z to 07:30Z
    }
    @Test fun halfHourGapUsesFirstValidWallMinute() {
        val band = bands(LocalDate(2026, 10, 4), "02:15:00", "03:00:00", "Australia/Lord_Howe").single()
        assertEquals(120.0, band.startMinute) // 02:30 is first valid wall time
        assertEquals(30.0, band.durationMinutes)
    }

    @Test fun skippedDayUsesSameBoundedMinuteWalkAsServer() {
        val zone = TimeZone.of("Pacific/Apia")
        val resolved = resolveAvailabilityLocal(LocalDateTime(2011, 12, 30, 0, 0, 45), zone)
        assertEquals(LocalDateTime(2011, 12, 31, 0, 0), resolved.toLocalDateTime(zone))
        assertEquals(kotlin.time.Instant.parse("2011-12-30T10:00:00Z"), resolved)
    }

    @Test fun dstAndOvernightUseElapsedMemberZoneMinutes() {
        val windows = listOf(Window(5, "22:00:00", "06:00:00"))
        val rows = listOf(AvailabilityResponse("01999999-0000-7000-8000-000000000001", 1, windows, emptyList()))
        val spring = availabilityBands(LocalDate(2026,3,8), TimeZone.of(member.tzid), rows, listOf(member), setOf("01999999-0000-7000-8000-000000000001"))
        assertEquals(1, spring.size)
        assertEquals(0.0, spring.single().startMinute)
        assertEquals(300.0, spring.single().durationMinutes) // 00–06 skips 02
        assertTrue(availabilityBands(LocalDate(2026,3,8), TimeZone.of(member.tzid), rows, listOf(member), emptySet()).isEmpty())
        val equal = rows.map { it.copy(windows = listOf(Window(6,"09:00:00","09:00:00"))) }
        assertTrue(availabilityBands(LocalDate(2026,3,8), TimeZone.of(member.tzid), equal, listOf(member), setOf("01999999-0000-7000-8000-000000000001")).isEmpty())
    }
}
