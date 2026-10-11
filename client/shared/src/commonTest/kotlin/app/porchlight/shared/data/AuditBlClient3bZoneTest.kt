package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.LocalDate
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.milliseconds

class AuditBlClient3bZoneTest {
    @Test fun occurrenceUsesOwnZoneAndAgendaUsesEachMembersZoneAtExactExpiryBoundary() = runTest {
        val now = Instant.parse("2026-03-01T00:00:00Z")
        val self = MemberRead(false, "self", "household", "Sam Rivera", "user", "#FABD2F", "S", "America/New_York", "manual", 1)
        val other = self.copy(id = "other", tzid = "Asia/Tokyo")
        data class Case(val mode: String, val occurrence: OccurrenceRead, val own: String, val other: String, val date: LocalDate? = null)
        val cases = listOf(
            Case("fixed", occurrenceRead("occ", "item").copy(dueAt = Instant.parse("2026-03-08T07:00:00Z")), "2026-03-08T07:00:00Z", "2026-03-08T07:00:00Z"),
            Case("floating", occurrenceRead("occ", "item").copy(localDue = "2026-03-08T02:30:00"), "2026-03-08T07:00:00Z", "2026-03-07T17:30:00Z"),
            Case("floating", occurrenceRead("occ", "item").copy(localDue = "2026-11-01T01:30:00"), "2026-11-01T05:30:00Z", "2026-10-31T16:30:00Z"),
            Case("date_only", occurrenceRead("occ", "item").copy(dueDate = LocalDate.parse("2026-11-01")), "2026-11-02T05:00:00Z", "2026-11-01T15:00:00Z", LocalDate.parse("2026-11-01")),
        )
        for (case in cases) {
            val cache = memoryCache { now }
            try {
                val ownExpiry = Instant.parse(case.own)
                val otherExpiry = Instant.parse(case.other)
                val item = itemRead("item").copy(timeMode = case.mode, tzid = if (case.mode == "fixed") "UTC" else null)
                cache.install(emptySnapshot(1).copy(items = listOf(item), occurrences = listOf(case.occurrence), members = listOf(self, other)))
                val agendas = listOf("self" to ownExpiry, "other" to otherExpiry).map { (member, expiry) ->
                    val row = agenda("occ", "item", member, case.date)
                    row.copy(agenda = row.agenda!!.copy(dueAt = expiry.takeIf { case.date == null }))
                }
                cache.replaceAgenda(agendas)
                for (cutoff in listOf(ownExpiry, otherExpiry).distinct().sorted()) {
                    cache.cleanup(cutoff + 180.days)
                    assertEquals(if (ownExpiry >= cutoff) 1 else 0, cache.view.value.occurrences.size, "${case.mode}: own boundary")
                    assertEquals(agendas.filter { (if (it.agenda!!.memberId == "self") ownExpiry else otherExpiry) >= cutoff }.map { it.agenda!!.memberId }.toSet(),
                        cache.view.value.agenda.map { it.agenda!!.memberId }.toSet(), "${case.mode}: member-local boundary")
                    cache.cleanup(cutoff + 180.days + 1.milliseconds)
                    assertEquals(if (ownExpiry > cutoff) 1 else 0, cache.view.value.occurrences.size)
                    assertEquals(agendas.count { (if (it.agenda!!.memberId == "self") ownExpiry else otherExpiry) > cutoff }, cache.view.value.agenda.size)
                }
            } finally { cache.close() }
        }
    }
}
