package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Duration.Companion.hours
import kotlin.time.Instant

class PrerolloutClientSyncBoundaryTest {
    private val f = SyncBundleFixtures
    private val zone = TimeZone.of("America/New_York")
    private val item = f.snapshot.items.single().copy(timeMode = "floating", tzid = null)
    private val point = f.snapshot.occurrences.single().copy(startsAt = null, endsAt = null, dueAt = null,
        localStart = null, localEnd = null, localDue = null, dueDate = null,
        isDetached = false, overrideLocalStart = null, overrideLocalEnd = null, overrideLocalDue = null, overrideDueDate = null)

    private fun window(from: String, to: String, inZone: TimeZone = zone): SyncWindow {
        val start = Instant.parse(from); val end = Instant.parse(to)
        return WireJson.decodeFromJsonElement(buildJsonObject {
            put("from", start.toString()); put("to", end.toString()); put("zone", inZone.id)
            put("local_from", start.toLocalDateTime(inZone).toString()); put("local_to", end.toLocalDateTime(inZone).toString())
        })
    }
    private suspend fun omitted(row: OccurrenceRead, bounds: SyncWindow, covered: Boolean, canonical: ItemRead = item) {
        val cache = memoryCache { Instant.parse("2026-01-01T00:00:00Z") }
        try {
            cache.install(f.snapshot.copy(items = listOf(canonical), occurrences = listOf(row)))
            assertEquals(listOf(row), cache.view.value.occurrences, "The omitted row must be present before reconciliation")
            val bundle = f.bundle(f.snapshot.copy(items = listOf(canonical), occurrences = emptyList(), scheduledReminders = emptyList()), emptyList(), emptyList())
            cache.installBundle(bundle.copy(coverage = bundle.coverage.copy(occurrences = bounds)))
            assertEquals(if (covered) emptyList() else listOf(row.id), cache.view.value.occurrences.map { it.id })
        } finally { cache.close() }
    }

    @Test fun springGapUsesFirstValidMinuteNotGapDurationAndPreservesCollapsedSpan() = runTest {
        val gap = window("2026-03-08T07:00:00Z", "2026-03-08T07:01:00Z")
        omitted(point.copy(localDue = "2026-03-08T02:30:15"), gap, true)
        omitted(point.copy(localStart = "2026-03-08T02:15:00", localEnd = "2026-03-08T02:45:00"),
            window("2026-03-08T07:10:00Z", "2026-03-08T07:11:00Z"), true)
    }

    @Test fun foldBoundsStayUtcAndFloatingPointChoosesEarlierInstant() = runTest {
        val row = point.copy(localDue = "2026-11-01T01:30:00")
        omitted(row, window("2026-11-01T05:30:00Z", "2026-11-01T05:30:00.000000001Z"), true)
        omitted(row, window("2026-11-01T06:30:00Z", "2026-11-01T07:00:00Z"), false)
        omitted(row, window("2026-11-01T05:00:00Z", "2026-11-01T05:30:00Z"), false)
    }

    @Test fun dateOnlyDaysUseSeparateMidnightsIncludingDstAndIgnoreAllDayFlag() = runTest {
        for ((date, end) in listOf("2026-03-08" to "2026-03-09T04:00:00Z", "2026-11-01" to "2026-11-02T05:00:00Z")) {
            val row = point.copy(dueDate = LocalDate.parse(date))
            val midnight = Instant.parse(end)
            omitted(row, window((midnight - 1.hours).toString(), end), true, item.copy(allDay = false))
            omitted(row, window(end, (midnight + 1.hours).toString()), false, item.copy(allDay = true))
        }
    }

    @Test fun detachedEffectiveAnchorOnlyAndFixedGeneratedSecondFoldIsNotReResolved() = runTest {
        val bounds = window("2026-11-01T06:30:00Z", "2026-11-01T07:00:00Z")
        val fixed = item.copy(timeMode = "fixed", tzid = zone.id)
        val generated = point.copy(dueAt = bounds.from, localDue = "2026-11-01T01:30:00")
        omitted(generated, bounds, true, fixed)
        omitted(generated.copy(isDetached = true, overrideLocalDue = "2026-11-01T01:45:00"), bounds, false, fixed)
        omitted(point.copy(isDetached = true, localDue = "2026-11-01T00:00:00", overrideLocalDue = "2026-11-01T01:30:00"),
            window("2026-11-01T05:30:00Z", "2026-11-01T06:00:00Z"), true)
        omitted(point.copy(isDetached = false, localDue = "2026-11-01T00:00:00", overrideLocalDue = "2026-11-01T01:30:00"),
            window("2026-11-01T05:30:00Z", "2026-11-01T06:00:00Z"), false)
    }

    @Test fun allDaySpansAndMixedLegacyAnchorsFollowPriorityAndHalfOpenOverlap() = runTest {
        val span = point.copy(localStart = "2026-03-07T00:00:00", localEnd = "2026-03-10T00:00:00")
        omitted(span, window("2026-03-08T07:00:00Z", "2026-03-09T04:00:00Z"), true, item.copy(allDay = true))
        omitted(span, window("2026-03-10T04:00:00Z", "2026-03-10T05:00:00Z"), false, item.copy(allDay = true))
        omitted(point.copy(dueAt = Instant.parse("2026-11-01T06:30:00Z"), localDue = "2026-11-01T00:00:00"),
            window("2026-11-01T06:30:00Z", "2026-11-01T07:00:00Z"), true)
    }

    @Test fun agendaUsesReportedMemberZoneAndPostgresLaterFoldMidnight() = runTest {
        // Member fixture is UTC; the scope, not stale /members data, supplies the effective zone.
        val havana = TimeZone.of("America/Havana")
        val bounds = window("2026-11-01T04:00:00Z", "2026-11-01T04:30:00Z", havana)
        val agenda = f.agenda.copy(agenda = f.agenda.agenda!!.copy(startsAt = null, endsAt = null, dueAt = null,
            dueDate = LocalDate.parse("2026-11-01")))
        val cache = memoryCache { f.now }
        try {
            cache.install(f.snapshot, projections = Projections(emptyList(), AgendaWindow(bounds.from, bounds.to, null, listOf(agenda))))
            val p = f.bundle(agenda = emptyList(), board = emptyList())
            val coverage: AgendaCoverage = WireJson.decodeFromJsonElement(buildJsonObject {
                put("member_id", agenda.agenda!!.memberId); put("from", bounds.from.toString()); put("to", bounds.to.toString())
                put("zone", havana.id); put("local_from", bounds.from.toLocalDateTime(havana).toString())
                put("local_to", bounds.to.toLocalDateTime(havana).toString()); put("complete", true)
            })
            cache.installBundle(p.copy(coverage = p.coverage.copy(itemSections = false, agenda = listOf(coverage)),
                agenda = listOf(SyncAgenda(coverage.memberId, bounds.from, bounds.to, emptyList()))))
            assertEquals(listOf(agenda), cache.view.value.agenda, "PostgreSQL resolves Havana midnight to 05:00Z, outside this window")
        } finally { cache.close() }
    }

    @Test fun detachedFixedGapSpanResolvesTogetherAndDateOverrideDoesNotUnionGeneratedDay() = runTest {
        val row = point.copy(isDetached = true, startsAt = Instant.parse("2026-03-07T12:00:00Z"),
            endsAt = Instant.parse("2026-03-07T13:00:00Z"), localStart = "2026-03-07T07:00:00", localEnd = "2026-03-07T08:00:00",
            overrideLocalStart = "2026-03-08T02:15:00", overrideLocalEnd = "2026-03-08T02:45:00")
        omitted(row, window("2026-03-08T07:10:00Z", "2026-03-08T07:11:00Z"), true, item.copy(timeMode = "fixed", tzid = zone.id))
        omitted(point.copy(isDetached = true, dueDate = LocalDate.parse("2026-03-08"), overrideDueDate = LocalDate.parse("2026-03-10")),
            window("2026-03-08T07:00:00Z", "2026-03-09T04:00:00Z"), false)
    }

    @Test fun undatedRowsNeedExplicitCoverage() {
        val bounds = window("2026-03-08T07:00:00Z", "2026-03-09T04:00:00Z")
        assertTrue(bundleOccurrenceCovered(point, item, bounds, true))
        assertFalse(bundleOccurrenceCovered(point, item, bounds, false))
    }

    @Test fun agendaDateGuardAndPostgresGapAndFoldMidnightsAreIndependentOfCorePolicy() {
        assertEquals(Instant.parse("2026-11-01T05:00:00Z"), postgresMidnight(LocalDate.parse("2026-11-01"), TimeZone.of("America/Havana")))
        assertEquals(Instant.parse("2026-11-01T04:00:00Z"), resolveOccurrenceLocal("2026-11-01T00:00:00", TimeZone.of("America/Havana")))
        assertEquals(Instant.parse("2026-09-06T04:00:00Z"), postgresMidnight(LocalDate.parse("2026-09-06"), TimeZone.of("America/Santiago")))
        val scope = AgendaCoverage(f.snapshot.memberSettings.memberId, Instant.parse("2026-11-06T12:00:00Z"),
            Instant.parse("2026-11-07T12:00:00Z"), true)
        val row = f.agenda.agenda!!.copy(startsAt = scope.from, endsAt = null, dueAt = null, dueDate = LocalDate.parse("2026-11-01"))
        assertFalse(bundleAgendaCovered(row, scope, zone), "SQL's date floor applies even when A uses starts_at")
        assertTrue(bundleAgendaCovered(row.copy(dueAt = scope.from), scope, null), "Resolved due instants need no date zone")
    }

    @Test fun malformedMetadataOrUnresolvableAnchorsFailClosedWithoutChangingCache() = runTest {
        val bounds = window("2026-11-01T05:00:00Z", "2026-11-01T07:00:00Z")
        for ((row, coverage) in listOf(point.copy(localDue = "2026-11-01T01:30:00") to bounds.copy(localFrom = "2026-11-01T03:00:00"),
            point.copy(localDue = "not-a-time") to bounds,
            point.copy(localStart = "2026-11-01T02:00:00", localEnd = "2026-11-01T01:00:00") to bounds)) {
            val cache = memoryCache { Instant.parse("2026-01-01T00:00:00Z") }
            try {
                cache.install(f.snapshot.copy(items = listOf(item), occurrences = listOf(row)))
                val before = cache.view.value
                val p = f.bundle(f.snapshot.copy(items = listOf(item), occurrences = emptyList(), scheduledReminders = emptyList()), emptyList(), emptyList())
                assertFailsWith<InvalidItemBundleCoverage> { cache.installBundle(p.copy(coverage = p.coverage.copy(occurrences = coverage))) }
                assertEquals(before, cache.view.value)
            } finally { cache.close() }
        }
    }

    @Test fun metadataFreeOlderServerStillFallsBackAtomicallyAtLocalBoundary() = runTest {
        val cache = memoryCache { f.now }
        try {
            cache.install(f.snapshot.copy(items = listOf(item), occurrences = listOf(point.copy(localDue = "2026-04-09T12:00:00"))))
            val before = cache.view.value
            assertFailsWith<ItemBundleNeedsSnapshot> { cache.installBundle(f.bundle(f.snapshot.copy(items = listOf(item), occurrences = emptyList()))) }
            assertEquals(before, cache.view.value)
        } finally { cache.close() }
    }
}
