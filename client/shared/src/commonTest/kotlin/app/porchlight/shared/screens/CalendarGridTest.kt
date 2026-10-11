package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.itemRead
import app.porchlight.shared.navigation.CalendarView
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.transport.WireJson
import kotlinx.datetime.*
import kotlin.test.*

class CalendarGridTest {
    private fun row(id: String, from: String, to: String, member: String = "self", allDay: Boolean = false) =
        itemRead(id).copy(allDay = allDay, agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"$member","occurrence_id":"$id","item_id":"$id","starts_at":"$from","ends_at":"$to","derived_at":"2026-10-06T12:00:00Z"}"""))
    private val utc = TimeZone.UTC
    @Test fun connectedOverlapsHaveStableColumnsButAdjacentItemsDoNotOverlap() {
        val rows = listOf(row("a", "2026-10-07T09:00:00Z", "2026-10-07T11:00:00Z"),
            row("b", "2026-10-07T10:00:00Z", "2026-10-07T12:00:00Z", "alex"),
            row("c", "2026-10-07T11:00:00Z", "2026-10-07T13:00:00Z"),
            row("d", "2026-10-07T13:00:00Z", "2026-10-07T14:00:00Z"))
        val window = calendarWindow(CalendarView.Day, LocalDate(2026,10,7), utc)
        val day = calendarGrid(window, rows.reversed(), setOf("self", "alex"), utc).days.single()
        assertEquals(listOf(0, 1, 0, 0), day.timed.map { it.column })
        assertEquals(listOf(2, 2, 2, 1), day.timed.map { it.columns })
        assertEquals(listOf(540.0, 600.0, 660.0, 780.0), day.timed.map { it.startMinute })
        assertEquals(listOf(120.0, 120.0, 120.0, 60.0), day.timed.map { it.durationMinutes })
        assertEquals(day, calendarGrid(window, rows, setOf("self", "alex"), utc).days.single())
    }
    @Test fun dstAxisIsElapsedTimeWithMissingAndRepeatedWallHours() {
        val zone = TimeZone.of("America/New_York")
        val spring = calendarGrid(calendarWindow(CalendarView.Day, LocalDate(2026,3,8), zone),
            listOf(row("gap", "2026-03-08T07:00:00Z", "2026-03-08T08:00:00Z")), setOf("self"), zone).days.single()
        assertEquals(23, spring.hours)
        assertEquals(120.0, spring.timed.single().startMinute)
        assertEquals("03:00", spring.timed.single().startLabel)
        assertFalse(spring.ticks.any { it.label.startsWith("02:00") })
        val fall = calendarGrid(calendarWindow(CalendarView.Day, LocalDate(2026,11,1), zone), emptyList(), setOf("self"), zone).days.single()
        assertEquals(25, fall.hours)
        val repeated = fall.ticks.filter { it.label.startsWith("01:00") }
        assertEquals(listOf(60.0, 120.0), repeated.map { it.minute })
        assertEquals(2, repeated.map { it.label }.distinct().size)
    }
    @Test fun allDaySpansUseOneLaneAcrossDaysAndZoneShiftsSplitTimedRows() {
        val window = calendarWindow(CalendarView.Week, LocalDate(2026,10,7), utc)
        val grid = calendarGrid(window, listOf(row("trip", "2026-10-05T00:00:00Z", "2026-10-08T00:00:00Z", allDay = true),
            row("other", "2026-10-06T00:00:00Z", "2026-10-07T00:00:00Z", allDay = true)), setOf("self"), utc)
        assertEquals(3, grid.allDay.single { it.entry.item.id == "trip" }.dayCount)
        assertEquals(1, grid.allDay.single { it.entry.item.id == "trip" }.firstDay)
        assertEquals(setOf(0, 1), grid.allDay.map { it.lane }.toSet())
        val shifted = calendarGrid(calendarWindow(CalendarView.Day, LocalDate(2026,10,6), TimeZone.of("America/Los_Angeles")),
            listOf(row("shift", "2026-10-07T01:00:00Z", "2026-10-07T02:00:00Z")), setOf("self"), TimeZone.of("America/Los_Angeles"))
        assertEquals("18:00", shifted.days.single().timed.single().startLabel)
        assertTrue(calendarDays(window, listOf(itemRead("no-agenda")), null, utc).all { it.timed.isEmpty() && it.allDay.isEmpty() })
    }
    @Test fun everyDayUsesItsOwnWallClockTicksInDstWeeks() {
        for ((zoneId, transition) in listOf("America/New_York" to "2026-03-08", "America/New_York" to "2026-11-01",
            "Europe/Berlin" to "2026-03-29", "Europe/Berlin" to "2026-10-25")) {
            val zone = TimeZone.of(zoneId)
            val window = calendarWindow(CalendarView.Week, LocalDate.parse(transition), zone)
            val rows = window.dates.flatMap { date -> listOf(0, 3, 9, 23).map { hour ->
                row("$date-$hour", LocalDateTime(date, LocalTime(hour, 0)).toInstant(zone).toString(),
                    LocalDateTime(date, LocalTime(hour, 30)).toInstant(zone).toString())
            } }
            val grid = calendarGrid(window, rows, setOf("self"), zone)
            assertEquals(7, grid.days.size)
            assertEquals(if (transition.contains("-03-")) 23 else 25, grid.days.first().hours)
            grid.days.forEach { day ->
                day.timed.forEach { entry ->
                    val tick = day.ticks.single { it.minute == entry.startMinute }
                    assertTrue(tick.label.startsWith(entry.startLabel), "$zoneId ${day.date}: $tick / $entry")
                }
                if (day.date != window.dates.first()) {
                    assertEquals(24, day.hours)
                    assertEquals(540.0, day.timed.single { it.startLabel == "09:00" }.startMinute)
                }
            }
            val repeatedHour = if (zoneId == "Europe/Berlin") "02:00" else "01:00"
            val ticks = grid.days.first().ticks.filter { it.label.startsWith(repeatedHour) }
            if (transition.contains("-03-")) assertFalse(grid.days.first().ticks.any { it.label.startsWith("02:00") })
            else { assertEquals(2, ticks.size); assertEquals(2, ticks.map { it.label }.distinct().size) }
        }
    }
    @Test fun shortBlocksUseTheirDrawnExtentForConnectedOverlapColumns() {
        val window = calendarWindow(CalendarView.Day, LocalDate(2026,10,7), utc)
        val rows = listOf(row("short", "2026-10-07T08:00:00Z", "2026-10-07T08:05:00Z"),
            row("close", "2026-10-07T08:10:00Z", "2026-10-07T08:40:00Z"),
            row("adjacent", "2026-10-07T08:40:00Z", "2026-10-07T09:00:00Z"))
        val day = calendarGrid(window, rows, setOf("self"), utc).days.single()
        assertEquals(listOf(0, 1, 0), day.timed.map { it.column })
        assertEquals(listOf(2, 2, 1), day.timed.map { it.columns })
        assertEquals(day, calendarGrid(window, rows.reversed(), setOf("self"), utc).days.single())
    }
    @Test fun visualOverlapUsesCurrentScaleAndHalfOpenTransitiveGroups() {
        val window = calendarWindow(CalendarView.Day, LocalDate(2026,10,7), utc)
        val rows = listOf(row("short", "2026-10-07T08:00:00Z", "2026-10-07T08:05:00Z"),
            row("close", "2026-10-07T08:10:00Z", "2026-10-07T08:11:00Z"),
            row("chain", "2026-10-07T08:25:00Z", "2026-10-07T08:26:00Z"),
            row("touching", "2026-10-07T08:44:30Z", "2026-10-07T09:00:00Z"))
        val normal = calendarGrid(window, rows, setOf("self"), utc).days.single().timed
        assertEquals(listOf(0, 1, 0, 0), normal.map { it.column })
        assertEquals(listOf(2, 2, 2, 1), normal.map { it.columns })
        assertEquals(listOf(5.0, 1.0, 1.0, 15.5), normal.map { it.durationMinutes })
        assertTrue(normal.all { it.displayDurationMinutes == 19.5 })
        val double = calendarGrid(window, rows, setOf("self"), utc, CalendarGridScale(2f)).days.single().timed
        assertEquals(normal.map { it.columns }, double.map { it.columns }) // Text and targets scale with the hour height.
        assertEquals(normal.map { it.displayDurationMinutes }, double.map { it.displayDurationMinutes })
        val closer = rows.take(2).map { if (it.id == "close") row("close", "2026-10-07T08:09:00Z", "2026-10-07T08:10:00Z") else it }
        assertEquals(listOf(0, 1), calendarGrid(window, closer, setOf("self"), utc, CalendarGridScale(2f)).days.single().timed.map { it.column })
    }
    @Test fun conflictKindsAndMaskingIncludeZoneTimeRange() {
        val hidden = WireJson.decodeFromString<ConflictRead>("""{"member_id":"self","occurrence_id":"o","other_item_id":null,"other_title":"Secret title must not leak","starts_at":"2026-10-07T12:00:00Z","ends_at":"2026-10-07T13:00:00Z","kind":"hard"}""")
        assertEquals("Conflict · Busy · 08:00–09:00", calendarConflictLabel(hidden, TimeZone.of("America/New_York")))
        assertEquals("Over capacity that day · 12:00–13:00", calendarConflictLabel(hidden.copy(kind = "load"), utc))
    }
}
