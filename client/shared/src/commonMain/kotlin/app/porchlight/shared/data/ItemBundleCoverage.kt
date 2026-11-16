package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import kotlinx.datetime.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Instant

internal data class AgendaBundleKey(val at: Instant, val occurrenceId: String) : Comparable<AgendaBundleKey> {
    override fun compareTo(other: AgendaBundleKey): Int = compareValuesBy(this, other, { it.at }, { it.occurrenceId })
}
internal fun AgendaRead.bundleKey(zone: TimeZone?): AgendaBundleKey? =
    (startsAt ?: dueAt ?: dueDate?.let { postgresMidnight(it, zone ?: throw ItemBundleNeedsSnapshot()) })
        ?.let { AgendaBundleKey(it, occurrenceId) }

internal data class AgendaBundleInterval(val scope: AgendaCoverage, val after: AgendaBundleKey?, val through: AgendaBundleKey?, val rows: List<ItemRead>)
internal data class CommentBundleInterval(val after: String?, val through: String?, val complete: Boolean, val rows: List<Comment>)
internal data class BundleIntervals(val agenda: List<AgendaBundleInterval>, val comments: List<CommentBundleInterval>)

/** Cursors are opaque: establish their exclusive lower bound from the preceding page, never decode a token. */
internal fun bundleIntervals(pages: List<ItemSyncRead>): BundleIntervals {
    val agendaEnds = mutableMapOf<String, AgendaBundleKey?>()
    val commentEnds = mutableMapOf<String, String?>()
    val agendas = mutableListOf<AgendaBundleInterval>()
    val comments = mutableListOf<CommentBundleInterval>()
    for (page in pages) {
        val c = page.coverage
        for (scope in c.agenda) {
            require(scope.cursor == null || scope.cursor in agendaEnds) { "Missing agenda predecessor" }
            require(scope.complete == (scope.nextCursor == null))
            val after = scope.cursor?.let { agendaEnds[it] }
            val rows = page.agenda.orEmpty().single { it.memberId == scope.memberId && it.from == scope.from && it.to == scope.to }.items
            val zone = scope.bundleZone()
            val keys = rows.map { requireNotNull(it.agenda).let { a -> require(a.memberId == scope.memberId); requireNotNull(a.bundleKey(zone)) } }
            require(keys.zipWithNext().all { (a, b) -> a < b } && keys.all { after == null || it > after })
            val through = keys.lastOrNull()
            agendas += AgendaBundleInterval(scope, after, through, rows)
            scope.nextCursor?.let { agendaEnds[it] = through ?: after }
        }
        if (c.comments) {
            require(c.commentsCursor == null || c.commentsCursor in commentEnds) { "Missing comment predecessor" }
            require(c.commentsComplete == (page.commentsNextCursor == null))
            val after = c.commentsCursor?.let { commentEnds[it] }
            val rows = requireNotNull(page.comments)
            require(rows.zipWithNext().all { (a, b) -> a.id < b.id } && rows.all { after == null || it.id > after })
            val through = rows.lastOrNull()?.id
            comments += CommentBundleInterval(after, through, c.commentsComplete, rows)
            page.commentsNextCursor?.let { commentEnds[it] = through ?: after }
        }
    }
    return BundleIntervals(agendas, comments)
}

/** Older servers omit zone metadata. Never guess a boundary membership from the device/member zone. */
class ItemBundleNeedsSnapshot : Exception("Item bundle coverage needs an authoritative snapshot")
internal class InvalidItemBundleCoverage : Exception("Item bundle coverage cannot be classified")

private fun bundleZone(from: Instant, to: Instant, id: String?, localFrom: String?, localTo: String?): TimeZone? {
    if (id == null || localFrom == null || localTo == null) return null
    return runCatching {
        val zone = TimeZone.of(id)
        // These are projections, not thresholds: retain the original instants on either side of a fold.
        require(from < to && LocalDateTime.parse(localFrom) == from.toLocalDateTime(zone) &&
            LocalDateTime.parse(localTo) == to.toLocalDateTime(zone))
        zone
    }.getOrElse { throw InvalidItemBundleCoverage() }
}
internal fun AgendaCoverage.bundleZone(): TimeZone? = bundleZone(from, to, zone, localFrom, localTo)

private fun resolved(value: String, zone: TimeZone): Instant =
    resolveOccurrenceLocal(value, zone) ?: throw InvalidItemBundleCoverage()
private fun resolvedSpan(start: String?, end: String?, zone: TimeZone): Pair<Instant, Instant>? {
    if (start == null || end == null) return null
    val span = resolveOccurrenceSpan(start, end, zone) ?: throw InvalidItemBundleCoverage()
    if (span.second <= span.first) throw InvalidItemBundleCoverage()
    return span
}
private fun coreMidnight(date: LocalDate, zone: TimeZone): Instant = resolved(date.atTime(0, 0).toString(), zone)

/** PostgreSQL AT TIME ZONE chooses the later fold instant and pre-transition offset in a gap.
 * This differs from core's earlier-fold/first-valid-minute rule, used only for occurrence anchors. */
internal fun postgresMidnight(date: LocalDate, zone: TimeZone): Instant {
    val local = date.atTime(0, 0)
    val nominal = local.toInstant(TimeZone.UTC)
    val candidates = listOf(nominal - 2.days, nominal, nominal + 2.days)
        .map { zone.offsetAt(it) }.distinct().map { local.toInstant(it) }
    return candidates.filter { it.toLocalDateTime(zone) == local }.maxOrNull() ?: candidates.max()
}

internal fun bundleAgendaCovered(row: AgendaRead, scope: AgendaCoverage, zone: TimeZone?): Boolean {
    val from = scope.from; val to = scope.to
    if (row.startsAt != null && row.endsAt != null && row.startsAt < to && row.endsAt > from) return true
    if (row.dueAt != null && row.dueAt >= from && row.dueAt < to) return true
    val date = row.dueDate ?: return false
    if (date < from.toLocalDateTime(TimeZone.UTC).date - DatePeriod(days = 3)) return false
    val effectiveZone = zone ?: throw ItemBundleNeedsSnapshot()
    val start = row.startsAt ?: row.dueAt ?: postgresMidnight(date, effectiveZone)
    return start < to && postgresMidnight(date + DatePeriod(days = 1), effectiveZone) > from
}

/** Snapshot coverage is NOT the occurrence-page start filter or the generated/detached candidate hull.
 * Classify only effective anchors; spans overlap, points use [from,to), and undated rows need explicit coverage. */
internal fun bundleOccurrenceCovered(row: OccurrenceRead, item: ItemRead?, window: SyncWindow, undated: Boolean): Boolean {
    val householdZone = bundleZone(window.from, window.to, window.zone, window.localFrom, window.localTo)
    var effective = row
    if (row.isDetached) {
        effective = row.copy(localStart = row.overrideLocalStart ?: row.localStart, localEnd = row.overrideLocalEnd ?: row.localEnd,
            localDue = row.overrideLocalDue ?: row.localDue, dueDate = row.overrideDueDate ?: row.dueDate)
        if (item?.timeMode == "fixed" && item.tzid != null) {
            val fixed = runCatching { TimeZone.of(item.tzid) }.getOrElse { throw InvalidItemBundleCoverage() }
            val span = resolvedSpan(effective.localStart, effective.localEnd, fixed)
            effective = effective.copy(startsAt = span?.first ?: effective.startsAt, endsAt = span?.second ?: effective.endsAt,
                dueAt = effective.localDue?.let { resolved(it, fixed) } ?: effective.dueAt)
        }
    }
    val zone = householdZone ?: TimeZone.UTC
    val localStart = effective.localStart ?: effective.localDue
    val hasLocalStart = effective.startsAt == null && effective.dueAt == null
    val start = effective.startsAt ?: effective.dueAt
        ?: localStart?.let { resolved(it, zone) }
        ?: effective.dueDate?.let { coreMidnight(it, zone) }
        ?: return undated
    val end = effective.endsAt
        ?: resolvedSpan(effective.localStart, effective.localEnd, zone)?.second
        ?: effective.localEnd?.let { resolved(it, zone) }
        ?: effective.dueDate?.let { coreMidnight(it + DatePeriod(days = 1), zone) }
    if (householdZone != null) return if (end == null) start >= window.from && start < window.to
        else start < window.to && end > window.from
    // Compatibility for old servers: safely classify rows away from local boundaries, otherwise snapshot.
    val startMargin = if (hasLocalStart) 1.days else kotlin.time.Duration.ZERO
    val endMargin = if (effective.endsAt == null && end != null) 1.days else kotlin.time.Duration.ZERO
    if (start + startMargin < window.to && (end?.let { it - endMargin > window.from } ?: (start - startMargin >= window.from))) return true
    if (start - startMargin >= window.to || (end?.let { it + endMargin <= window.from } ?: (start + startMargin < window.from))) return false
    throw ItemBundleNeedsSnapshot()
}
