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
    val open = rows.filter { it.occurrence?.let { o -> o.status !in setOf("done", "skipped") && o.disposition != "expired" } == true }
        .distinctBy { it.occurrence!!.id }
        .sortedWith(compareBy<ItemRead>({ date(it) }, { it.agenda?.dueAt ?: it.agenda?.startsAt ?: it.occurrence?.effective(it)?.dueAt ?: it.occurrence?.effective(it)?.startsAt }, { it.id }, { it.occurrence!!.id }))
    val scoped = open.filter { settings.scope == SettingsScope.household || belongsTo(it, member) }
    fun overdue(row: ItemRead) = row.occurrence?.disposition == "overdue" || date(row)?.let { it < day } == true
    return settings.todaySections.map { section -> TodayGroup(section, when (section) {
        TodaySection.overdue -> scoped.filter(::overdue)
        TodaySection.today -> scoped.filter { date(it) == day && !overdue(it) }
        TodaySection.this_week -> scoped.filter { date(it)?.let { d -> d > day && d < end } == true && !overdue(it) }
        TodaySection.needs_owner -> open.filter { it.assignees.isEmpty() && date(it)?.let { d -> d >= day && d < end } == true }
        TodaySection.pinned_list -> emptyList()
    }, if (section == TodaySection.pinned_list) settings.pinnedListId else null) }
}
