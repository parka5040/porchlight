package app.porchlight.shared.screens

import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.data.effective
import kotlinx.datetime.*
import kotlin.time.Instant

internal expect fun localizedDate(date: LocalDate): String
internal expect fun localizedDateTime(date: LocalDateTime): String

/** Agenda instants are already resolved for the member; floating anchors stay wall-clock values. */
fun itemTimeLabel(row: ItemRead, zone: TimeZone): String {
    val o = row.occurrence?.effective(row)
    val a = row.agenda
    val date = a?.dueDate ?: o?.overrideDueDate ?: o?.dueDate ?: row.dueDate
    fun wall(local: String?, instant: Instant?): LocalDateTime? =
        if (row.timeMode == "floating") local?.let { runCatching { LocalDateTime.parse(it) }.getOrNull() }
        else instant?.toLocalDateTime(zone)
    val due = wall(o?.overrideLocalDue ?: o?.localDue ?: row.localDue, a?.dueAt ?: o?.dueAt ?: row.dueAt)
    val start = wall(o?.overrideLocalStart ?: o?.localStart ?: row.localStart, a?.startsAt ?: o?.startsAt ?: row.startsAt)
    val end = wall(o?.overrideLocalEnd ?: o?.localEnd ?: row.localEnd, a?.endsAt ?: o?.endsAt ?: row.endsAt)
    val text = when {
        date != null -> localizedDate(date)
        due != null -> localizedDateTime(due)
        start != null -> if (row.allDay) localizedDate(start.date) else localizedDateTime(start) + (end?.let { " – ${localizedDateTime(it)}" } ?: "")
        else -> "No date"
    }
    return if (o?.disposition == "overdue" && row.missedPolicy == "carry_over") "Was due $text · carried over" else text
}

fun recurrenceLabel(row: ItemRead): String {
    if (row.recurrenceMode == "completion") {
        val interval = row.completionInterval ?: return "Repeats after completion"
        val parts = buildList {
            fun part(n: Long, unit: String) { if (n != 0L) add("$n $unit${if (n == 1L) "" else "s"}") }
            part(interval.months.toLong(), "month"); part(interval.days.toLong(), "day")
            if (interval.microseconds != 0L) {
                val seconds = interval.microseconds / 1_000_000
                part(seconds / 3600, "hour"); part(seconds % 3600 / 60, "minute"); part(seconds % 60, "second")
                part(interval.microseconds % 1_000_000, "microsecond")
            }
        }
        return "${parts.joinToString(", ")} after completion"
    }
    val rule = row.rrule ?: return "Does not repeat"
    val values = rule.removePrefix("RRULE:").split(';').mapNotNull { it.split('=', limit = 2).takeIf { p -> p.size == 2 }?.let { p -> p[0] to p[1] } }.toMap()
    val unit = when (values["FREQ"]) { "DAILY" -> "day"; "WEEKLY" -> "week"; "MONTHLY" -> "month"; "YEARLY" -> "year"; else -> return "Repeats" }
    val n = values["INTERVAL"]?.toIntOrNull() ?: 1
    val days = mapOf("MO" to "Monday", "TU" to "Tuesday", "WE" to "Wednesday", "TH" to "Thursday", "FR" to "Friday", "SA" to "Saturday", "SU" to "Sunday")
    val byDay = values["BYDAY"]?.split(',')?.joinToString(", ") { token ->
        val day = days[token.takeLast(2)] ?: token
        val ordinal = token.dropLast(2)
        val order = mapOf("1" to "first", "2" to "second", "3" to "third", "4" to "fourth", "5" to "fifth", "-1" to "last", "-2" to "second-last", "-3" to "third-last", "-4" to "fourth-last", "-5" to "fifth-last")
        if (ordinal.isEmpty()) day else "${order[ordinal] ?: ordinal} $day"
    }
    return "Every ${if (n == 1) unit else "$n ${unit}s"}" +
        (byDay?.let { " on $it" } ?: values["BYMONTHDAY"]?.let { " on day $it" }.orEmpty()) +
        values["COUNT"]?.let { " · $it occurrences" }.orEmpty() +
        values["UNTIL"]?.let { until ->
            val date = runCatching { LocalDate(until.take(4).toInt(), until.substring(4, 6).toInt(), until.substring(6, 8).toInt()) }.getOrNull()
            " · until ${date?.let(::localizedDate) ?: "end date"}"
        }.orEmpty()
}

internal expect fun addressClipEntry(text: String): androidx.compose.ui.platform.ClipEntry
