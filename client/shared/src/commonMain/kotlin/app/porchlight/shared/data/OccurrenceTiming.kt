package app.porchlight.shared.data

import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.api.models.OccurrenceRead
import kotlinx.datetime.*
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Instant

/** Snapshot/board rows retain generated anchors. Normalize a copy, never the recurrence identity in storage. */
fun OccurrenceRead.effective(item: ItemRead?, fallbackZone: TimeZone = TimeZone.UTC): OccurrenceRead {
    return effective(if (item?.timeMode == "fixed" && item.tzid == null) "floating" else item?.timeMode ?: "fixed", item?.tzid?.let { runCatching { TimeZone.of(it) }.getOrNull() } ?: fallbackZone)
}

fun OccurrenceRead.effective(timeMode: String, zone: TimeZone): OccurrenceRead {
    val due = overrideLocalDue ?: localDue
    val start = overrideLocalStart ?: localStart
    val end = overrideLocalEnd ?: localEnd
    fun instant(override: String?, original: Instant?): Instant? =
        if (timeMode == "floating" || override == null) original else resolveOccurrenceLocal(override, zone) ?: original
    var effectiveStart = instant(overrideLocalStart, startsAt)
    var effectiveEnd = instant(overrideLocalEnd, endsAt)
    // R10: a gap may collapse the resolved span; preserve its positive nominal wall duration.
    if (timeMode != "floating" && (overrideLocalStart != null || overrideLocalEnd != null) && start != null && end != null) {
        resolveOccurrenceSpan(start, end, zone)?.let { (s, e) ->
            effectiveStart = s
            effectiveEnd = e
        }
    }
    return copy(dueDate = overrideDueDate ?: dueDate, localDue = due, localStart = start, localEnd = end,
        dueAt = instant(overrideLocalDue, dueAt), startsAt = effectiveStart, endsAt = effectiveEnd)
}

/** Same policy as core time::resolve_local: earlier fold offset, first valid minute after a gap. */
internal fun resolveOccurrenceLocal(value: String, zone: TimeZone): Instant? = runCatching {
    val local = LocalDateTime.parse(value)
    val resolved = local.toInstant(zone)
    if (resolved.toLocalDateTime(zone) == local) resolved else {
        val minute = LocalDateTime(local.year, local.month, local.day, local.hour, local.minute).toInstant(TimeZone.UTC)
        (1..48 * 60).firstNotNullOf { n ->
            val next = (minute + n.minutes).toLocalDateTime(TimeZone.UTC)
            next.toInstant(zone).takeIf { it.toLocalDateTime(zone) == next }
        }
    }
}.getOrNull()

/** Resolve endpoints together: a collapsed/inverted DST gap keeps the nominal wall duration (R10). */
internal fun resolveOccurrenceSpan(start: String?, end: String?, zone: TimeZone): Pair<Instant, Instant>? {
    val s = start?.let { resolveOccurrenceLocal(it, zone) } ?: return null
    val e = end?.let { resolveOccurrenceLocal(it, zone) } ?: return null
    val nominal = runCatching { LocalDateTime.parse(end).toInstant(TimeZone.UTC) - LocalDateTime.parse(start).toInstant(TimeZone.UTC) }.getOrNull()
    return s to if (e <= s && nominal != null && nominal.isPositive()) s + nominal else e
}
