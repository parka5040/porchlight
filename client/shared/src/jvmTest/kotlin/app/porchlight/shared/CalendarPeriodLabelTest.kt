package app.porchlight.shared

import app.porchlight.shared.navigation.CalendarView
import app.porchlight.shared.screens.calendar.calendarPeriodLabel
import app.porchlight.shared.screens.calendar.calendarZoneLabel
import kotlinx.datetime.LocalDate
import kotlinx.datetime.TimeZone
import kotlin.test.Test
import kotlin.test.assertEquals

class CalendarPeriodLabelTest {
    @Test fun labelsUseInclusiveWindowDatesAcrossMonthsYearsAndLeapDays() {
        val zone = TimeZone.of("America/New_York")
        listOf(
            Triple(CalendarView.Day, "2026-10-09", "Fri, Oct 9, 2026"),
            Triple(CalendarView.Week, "2026-10-09", "Oct 4 – 10, 2026"),
            Triple(CalendarView.Agenda, "2026-10-09", "Oct 9 – 15, 2026"),
            Triple(CalendarView.Agenda, "2026-10-30", "Oct 30 – Nov 5, 2026"),
            Triple(CalendarView.Week, "2027-01-01", "Dec 27, 2026 – Jan 2, 2027"),
            Triple(CalendarView.Day, "2028-02-29", "Tue, Feb 29, 2028"),
            Triple(CalendarView.Month, "2026-10-09", "Oct 2026"),
        ).forEach { (view, date, expected) -> assertEquals(expected, calendarPeriodLabel(view, LocalDate.parse(date), zone)) }
    }

    @Test fun cityZonesAreHumanReadableAndNonCityZonesStayUnambiguous() {
        assertEquals("New York time", calendarZoneLabel(TimeZone.of("America/New_York")))
        assertEquals("Los Angeles time", calendarZoneLabel(TimeZone.of("America/Los_Angeles")))
        assertEquals("UTC", calendarZoneLabel(TimeZone.UTC))
        assertEquals("Etc/GMT+5", calendarZoneLabel(TimeZone.of("Etc/GMT+5")))
    }
}
