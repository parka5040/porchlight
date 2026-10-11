package app.porchlight.shared.screens.calendar

import app.porchlight.shared.api.models.*
import app.porchlight.shared.navigation.CalendarView
import kotlinx.datetime.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.hours

/** Half-open member-zone window. Week starts Sunday, as in DesktopCalendar. */
data class CalendarWindow(val dates: List<LocalDate>, val from: Instant, val to: Instant)
data class CalendarEntry(val item: ItemRead, val startLabel: String, val endLabel: String,
    val startMinute: Double = 0.0, val durationMinutes: Double = 0.0, val column: Int = 0, val columns: Int = 1,
    val displayDurationMinutes: Double = durationMinutes,
    val memberIds: Set<String> = setOfNotNull(item.agenda?.memberId))
/** Pixel-free drawing metrics shared by overlap layout and Compose; 52dp leaves a 48dp padded target. */
data class CalendarGridScale(val fontScale: Float = 1f) {
    val hourHeightDp: Float get() = 160f * fontScale.coerceAtLeast(1f)
    val minimumDurationMinutes: Double get() = 52.0 * fontScale.coerceAtLeast(1f) * 60.0 / hourHeightDp
}
data class CalendarTick(val minute: Double, val label: String)
data class CalendarDay(val date: LocalDate, val hours: Long, val allDay: List<CalendarEntry>, val timed: List<CalendarEntry>,
    val ticks: List<CalendarTick> = emptyList())
data class CalendarAllDay(val entry: CalendarEntry, val firstDay: Int, val dayCount: Int, val lane: Int)
data class CalendarGrid(val days: List<CalendarDay>, val allDay: List<CalendarAllDay>, val scale: CalendarGridScale = CalendarGridScale())

fun calendarWindow(view: CalendarView, date: LocalDate, zone: TimeZone): CalendarWindow {
    val first = when (view) {
        CalendarView.Month -> LocalDate(date.year, date.month, 1).sunday()
        CalendarView.Week -> date.sunday()
        else -> date
    }
    val end = when (view) {
        CalendarView.Month -> {
            val nextMonth = LocalDate(date.year, date.month, 1).plus(DatePeriod(months = 1))
            if (nextMonth.dayOfWeek == DayOfWeek.SUNDAY) nextMonth else nextMonth.sunday().plus(DatePeriod(days = 7))
        }
        CalendarView.Week, CalendarView.Agenda -> first.plus(DatePeriod(days = 7))
        CalendarView.Day -> first.plus(DatePeriod(days = 1))
    }
    val dates = generateSequence(first) { it.plus(DatePeriod(days = 1)) }.takeWhile { it < end }.toList()
    return CalendarWindow(dates, first.atStartOfDayIn(zone), end.atStartOfDayIn(zone))
}
private val monthNames = listOf("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")
private val dayNames = listOf("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
private fun LocalDate.monthLabel() = monthNames[month.ordinal]
private fun LocalDate.monthDayLabel(includeYear: Boolean = false) =
    "${monthLabel()} $day" + if (includeYear) ", $year" else ""

/** Day cards share the period's month/day wording; only a cross-year window repeats the year. */
fun calendarDayHeading(date: LocalDate, includeYear: Boolean): String =
    "${date.dayOfWeek.name.lowercase().replaceFirstChar { it.uppercase() }}, ${date.monthDayLabel(includeYear)}"

/** English-only presentation of the same inclusive dates used by the half-open query window. */
fun calendarPeriodLabel(view: CalendarView, date: LocalDate, zone: TimeZone): String {
    if (view == CalendarView.Day) return "${dayNames[date.dayOfWeek.isoDayNumber - 1]}, ${date.monthDayLabel(includeYear = true)}"
    if (view == CalendarView.Month) return "${date.monthLabel()} ${date.year}"
    val dates = calendarWindow(view, date, zone).dates
    val first = dates.first()
    val last = dates.last()
    return when {
        first.year != last.year -> "${first.monthDayLabel(includeYear = true)} – ${last.monthDayLabel(includeYear = true)}"
        first.month != last.month -> "${first.monthDayLabel()} – ${last.monthDayLabel(includeYear = true)}"
        else -> "${first.monthDayLabel()} – ${last.day}, ${last.year}"
    }
}

fun calendarZoneLabel(zone: TimeZone): String = if (zone.id.startsWith("Etc/") || '/' !in zone.id) zone.id
    else zone.id.substringAfterLast('/').replace('_', ' ') + " time"

private fun LocalDate.sunday() = minus(DatePeriod(days = dayOfWeek.isoDayNumber % 7))
fun adjacentDate(view: CalendarView, date: LocalDate, direction: Int): LocalDate = when (view) {
    CalendarView.Month -> LocalDate(date.year, date.month, 1).plus(DatePeriod(months = direction))
    CalendarView.Day -> date.plus(DatePeriod(days = direction))
    else -> date.plus(DatePeriod(days = direction * 7))
}
private fun Instant.clockLabel(zone: TimeZone): String = toLocalDateTime(zone).time.let {
    "${it.hour.toString().padStart(2, '0')}:${it.minute.toString().padStart(2, '0')}"
}
fun calendarConflictLabel(conflict: ConflictRead, zone: TimeZone): String =
    (if (conflict.kind == "load") "Over capacity that day" else "Conflict · ${if (conflict.otherItemId == null) "Busy" else conflict.otherTitle}") +
        " · ${conflict.startsAt.clockLabel(zone)}–${conflict.endsAt.clockLabel(zone)}"
fun calendarEntryLabels(entry: CalendarEntry, zone: TimeZone, allDay: Boolean = false): List<String> = buildList {
    add("${if (entry.item.timeMode == "floating") "Floating" else "Fixed"} · ${if (allDay) "All day" else entry.startLabel + if (entry.endLabel.isNotEmpty()) "–${entry.endLabel}" else ""}")
    if (entry.item.pendingDerivation) add("Pending derivation")
    entry.item.conflicts.orEmpty().filter { it.memberId in entry.memberIds }.map { calendarConflictLabel(it, zone) }.distinct().forEach { add(it) }
}

/** Only resolved server agenda instants are positioned, including overlays. No local floating resolution. */
fun calendarDays(window: CalendarWindow, rows: List<ItemRead>, memberId: String?, zone: TimeZone): List<CalendarDay> =
    calendarGrid(window, rows, setOfNotNull(memberId), zone).days
fun calendarGrid(window: CalendarWindow, rows: List<ItemRead>, memberIds: Set<String>, zone: TimeZone,
    scale: CalendarGridScale = CalendarGridScale()): CalendarGrid {
    val overlays = rows.filter { it.deletedAt == null && it.agenda != null && it.agenda.memberId in memberIds }
        .distinctBy { it.agenda!!.memberId to it.agenda.occurrenceId }
        // Floating occurrences can resolve differently per member; only coincident slots share a card.
        .groupBy { it.agenda!!.let { a -> listOf(a.occurrenceId, a.startsAt, a.endsAt, a.dueAt, a.dueDate) } }
        .values.map { shared ->
            shared.first().copy(conflicts = shared.flatMap { it.conflicts.orEmpty() }.distinct()) to
                shared.map { it.agenda!!.memberId }.toSet()
        }
    val visible = overlays.map { it.first }
    val days = window.dates.map { date ->
        val start = date.atStartOfDayIn(zone)
        val end = date.plus(DatePeriod(days = 1)).atStartOfDayIn(zone)
        val allDay = mutableListOf<CalendarEntry>()
        val timed = mutableListOf<CalendarEntry>()
        overlays.forEach { (item, sharedMembers) ->
            val agenda = item.agenda!!
            val from = agenda.startsAt ?: agenda.dueAt
            val until = agenda.endsAt
            val overlaps = when {
                from == null -> agenda.dueDate == date
                until != null && until > from -> from < end && until > start
                else -> from >= start && from < end
            }
            if (overlaps) {
                val clippedStart = from?.let { maxOf(it, start) } ?: start
                // A point/due-only entry uses one hour of display space, never a fabricated server end label.
                val clippedEnd = minOf(until?.takeIf { it > clippedStart } ?: (clippedStart + 1.hours), end)
                val entry = CalendarEntry(item, from?.let { clippedStart.clockLabel(zone) } ?: "All day",
                    until?.let { if (it >= end) "24:00" else it.clockLabel(zone) } ?: "",
                    (clippedStart - start).inWholeMilliseconds / 60000.0, (clippedEnd - clippedStart).inWholeMilliseconds / 60000.0,
                    memberIds = sharedMembers)
                if (item.allDay || agenda.dueDate != null && from == null) allDay += entry
                else {
                    val drawnMinutes = maxOf(entry.durationMinutes, scale.minimumDurationMinutes)
                    // Keep the minimum touch target inside the day; the label retains its actual server time.
                    timed += entry.copy(startMinute = minOf(entry.startMinute, (end - start).inWholeMinutes - drawnMinutes),
                        displayDurationMinutes = drawnMinutes)
                }
            }
        }
        val hours = (end - start).inWholeHours
        val ticks = (0 until hours).map { hour ->
            val instant = start + hour.hours
            // Offset disambiguates the two 01:00 ticks on a fall-back day.
            CalendarTick(hour * 60.0, instant.clockLabel(zone) + if (hours != 24L) " ${zone.offsetAt(instant)}" else "")
        }
        CalendarDay(date, hours, allDay.sortedBy { it.item.title }, overlapColumns(timed), ticks)
    }
    val spans = visible.flatMap { row ->
        val indices = days.indices.filter { index -> days[index].allDay.any { it.item.agenda == row.agenda } }
        if (indices.isEmpty()) emptyList() else listOf(CalendarAllDay(days[indices.first()].allDay.first { it.item.agenda == row.agenda }, indices.first(), indices.size, 0))
    }.sortedWith(compareBy<CalendarAllDay> { it.firstDay }.thenByDescending { it.dayCount }.thenBy { it.entry.item.id }.thenBy { it.entry.item.agenda!!.memberId })
    val laneEnds = mutableListOf<Int>()
    return CalendarGrid(days, spans.map { span ->
        val lane = laneEnds.indexOfFirst { it <= span.firstDay }.let { if (it < 0) laneEnds.size else it }
        if (lane == laneEnds.size) laneEnds.add(span.firstDay + span.dayCount) else laneEnds[lane] = span.firstDay + span.dayCount
        span.copy(lane = lane)
    }, scale)
}

/** Connected DRAWN overlap groups share a width; touching half-open intervals may reuse a column. */
private fun overlapColumns(entries: List<CalendarEntry>): List<CalendarEntry> {
    val sorted = entries.sortedWith(compareBy<CalendarEntry> { it.startMinute }.thenByDescending { it.durationMinutes }
        .thenBy { it.item.id }.thenBy { it.item.agenda!!.memberId })
    val output = mutableListOf<CalendarEntry>()
    var group = mutableListOf<CalendarEntry>()
    var groupEnd = 0.0
    fun flush() {
        val ends = mutableListOf<Double>()
        val placed = group.map { entry ->
            val column = ends.indexOfFirst { it <= entry.startMinute }.let { if (it < 0) ends.size else it }
            if (column == ends.size) ends.add(entry.startMinute + entry.displayDurationMinutes) else ends[column] = entry.startMinute + entry.displayDurationMinutes
            entry.copy(column = column)
        }
        output += placed.map { it.copy(columns = ends.size) }
        group = mutableListOf()
    }
    sorted.forEach { entry ->
        if (group.isNotEmpty() && entry.startMinute >= groupEnd) flush()
        if (group.isEmpty()) groupEnd = 0.0
        group += entry
        groupEnd = maxOf(groupEnd, entry.startMinute + entry.displayDurationMinutes)
    }
    if (group.isNotEmpty()) flush()
    return output
}

