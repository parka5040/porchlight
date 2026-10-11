package app.porchlight.shared.screens.today

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.data.effective
import kotlinx.datetime.*
import kotlin.time.Instant

val defaultTodaySettings = TodaySettings(listOf(TodaySection.overdue, TodaySection.today, TodaySection.this_week, TodaySection.needs_owner), SettingsScope.mine, 7)
data class TodayGroup(val kind: TodaySection, val rows: List<ItemRead>, val pinnedListId: String? = null)
fun belongsTo(item: ItemRead, member: String) = item.ownerId == member || item.subjectMemberId == member || member in item.assignees
fun TodaySection.label(days: Int): String = when (this) {
    TodaySection.overdue -> "Overdue"; TodaySection.today -> "Today"; TodaySection.this_week -> "This week ($days days)"
    TodaySection.needs_owner -> "Needs an owner"; TodaySection.pinned_list -> "Pinned list"
}
/** Occurrences, not the board's one-per-series projection. Prefer server-resolved member agenda anchors. */
fun todayRows(view: CacheView, member: String): List<ItemRead> {
    val agenda = view.agenda.filter { it.agenda?.memberId == member }.associateBy { it.agenda?.occurrenceId }
    val items = view.items.associateBy { it.id }
    return view.occurrences.mapNotNull { occ ->
        (agenda[occ.id] ?: items[occ.itemId])?.copy(occurrence = occ)
    }
}
fun todaySections(rows: List<ItemRead>, settings: TodaySettings, member: String, zone: TimeZone, now: Instant): List<TodayGroup> {
    val day = now.toLocalDateTime(zone).date
    val end = day.plus(settings.thisWeekDays, DateTimeUnit.DAY)
    fun date(row: ItemRead): LocalDate? {
        val a = row.agenda
        val o = row.occurrence?.effective(row)
        if (a != null) return a.dueDate ?: (a.dueAt ?: a.startsAt)?.toLocalDateTime(zone)?.date
        return o?.dueDate ?: (o?.dueAt ?: o?.startsAt)?.toLocalDateTime(zone)?.date
            ?: (o?.localDue ?: o?.localStart)?.let { runCatching { LocalDateTime.parse(it).date }.getOrNull() }
    }
    fun appointmentBounds(row: ItemRead): Pair<Instant, Instant>? {
        if (row.kind != "appointment") return null
        val o = row.occurrence?.effective(row, zone) ?: return null
        // Detached local overrides win, but must resolve as one span rather than collapse in a gap.
        if (row.allDay || row.timeMode == "floating") {
            app.porchlight.shared.data.resolveOccurrenceSpan(o.localStart, o.localEnd, zone)?.let { return it }
        }
        val start = o.startsAt ?: row.agenda?.startsAt
        val finish = o.endsAt ?: row.agenda?.endsAt
        return if (start != null && finish != null) start to finish else null
    }
    fun disposition(row: ItemRead): String? {
        val agenda = row.agenda?.takeIf { it.memberId == member }
        // An older server's agenda still proves recipient eligibility. Never let
        // the snapshot's owner-zone disposition override that member projection.
        if (agenda != null) return agenda.disposition ?: "open"
        if (row.timeMode == "floating" && belongsTo(row, member)) {
            // Undated work has no agenda window anchor. A series needs its member
            // projection (carry-over depends on the other occurrences), never the
            // owner-zone master fallback. Single work can resolve its own boundary.
            if (date(row) == null) return "open"
            if (row.recurrenceMode != null) return null
            val o = row.occurrence?.effective(row, zone) ?: return null
            val boundary = appointmentBounds(row)?.second
                ?: o.localDue?.let { app.porchlight.shared.data.resolveOccurrenceLocal(it, zone) }
                ?: o.dueDate?.plus(1, DateTimeUnit.DAY)?.atStartOfDayIn(zone)
            return if (boundary != null && boundary < now) {
                if (row.missedPolicy == "skip") "expired" else "overdue"
            } else "open"
        }
        return row.occurrence?.disposition
    }
    val open = rows.filter { row -> row.occurrence?.let { o -> o.status !in setOf("done", "skipped") && disposition(row) in setOf("open", "overdue") } == true }
        .distinctBy { it.occurrence!!.id }
        .sortedWith(compareBy<ItemRead>({ date(it) }, { it.agenda?.dueAt ?: it.agenda?.startsAt ?: it.occurrence?.effective(it)?.dueAt ?: it.occurrence?.effective(it)?.startsAt }, { it.id }, { it.occurrence!!.id }))
    val scoped = open.filter { settings.scope == SettingsScope.household || belongsTo(it, member) }
    fun overdue(row: ItemRead): Boolean = appointmentBounds(row)?.let { (_, finish) ->
        finish <= now
    } ?: (disposition(row) == "overdue" || date(row)?.let { it < day } == true)
    fun today(row: ItemRead): Boolean = appointmentBounds(row)?.let { (start, finish) ->
        start < day.plus(1, DateTimeUnit.DAY).atStartOfDayIn(zone) && finish > day.atStartOfDayIn(zone)
    } ?: (date(row) == day)
    return settings.todaySections.map { section -> TodayGroup(section, when (section) {
        TodaySection.overdue -> scoped.filter(::overdue)
        TodaySection.today -> scoped.filter { today(it) && !overdue(it) }
        TodaySection.this_week -> scoped.filter { date(it)?.let { d -> d > day && d < end } == true && !overdue(it) }
        TodaySection.needs_owner -> open.filter { it.assignees.isEmpty() && date(it)?.let { d -> d >= day && d < end } == true }
        TodaySection.pinned_list -> emptyList()
    }, if (section == TodaySection.pinned_list) settings.pinnedListId else null) }
}
