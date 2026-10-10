package app.porchlight.shared.screens.calendar

import app.porchlight.shared.api.models.*
import kotlinx.datetime.*
import kotlin.time.Instant

/** Mirrors server/core/time.rs resolve_local: earlier fold; first valid wall minute after a gap.
 * Round-tripping detects gaps without platform-only zone rules. kotlinx-datetime already chooses
 * the earlier fold, but its default gap conversion shifts by the gap length, which is not our policy.
 */
internal fun resolveAvailabilityLocal(local: LocalDateTime, zone: TimeZone): Instant {
    val resolved = local.toInstant(zone)
    if (resolved.toLocalDateTime(zone) == local) return resolved
    val minuteStart = LocalDateTime(local.date, LocalTime(local.hour, local.minute)).toInstant(TimeZone.UTC)
    // Same 48-hour bound and whole-minute walk as the server (including Apia's skipped day).
    for (minute in 1..48 * 60) {
        val candidate = minuteStart.plus(minute.toLong(), DateTimeUnit.MINUTE).toLocalDateTime(TimeZone.UTC)
        val instant = candidate.toInstant(zone)
        if (instant.toLocalDateTime(zone) == candidate) return instant
    }
    error("Time zone gap exceeds 48 hours")
}

data class AvailabilityBand(val memberId: String, val label: String, val startMinute: Double, val durationMinutes: Double)
/** Resolve member-local windows, including yesterday's overnight spill, onto the elapsed calendar day axis. */
fun availabilityBands(date: LocalDate, axisZone: TimeZone, availability: List<AvailabilityResponse>, members: List<MemberRead>, visible: Set<String>): List<AvailabilityBand> {
    val from = date.atStartOfDayIn(axisZone)
    val to = date.plus(1, DateTimeUnit.DAY).atStartOfDayIn(axisZone)
    return availability.filter { it.memberId in visible }.flatMap { row ->
        val member = members.find { it.id == row.memberId } ?: return@flatMap emptyList()
        val zone = runCatching { TimeZone.of(member.tzid) }.getOrElse { return@flatMap emptyList() }
        val localDate = from.toLocalDateTime(zone).date
        (-1..1).flatMap { offset ->
            val startDate = localDate.plus(offset, DateTimeUnit.DAY)
            row.windows.filter { it.weekday == startDate.dayOfWeek.ordinal }.mapNotNull { window ->
                val start = runCatching { LocalTime.parse(window.start) }.getOrNull() ?: return@mapNotNull null
                val end = runCatching { LocalTime.parse(window.end) }.getOrNull() ?: return@mapNotNull null
                if (start == end) return@mapNotNull null
                val endDate = if (end < start) startDate.plus(1, DateTimeUnit.DAY) else startDate
                val a = maxOf(resolveAvailabilityLocal(LocalDateTime(startDate, start), zone), from)
                val b = minOf(resolveAvailabilityLocal(LocalDateTime(endDate, end), zone), to)
                if (a >= b) null else AvailabilityBand(member.id, "Available ${member.displayName}: $start – $end (${zone.id})", (a - from).inWholeSeconds / 60.0, (b - a).inWholeSeconds / 60.0)
            }
        }
    }
}
