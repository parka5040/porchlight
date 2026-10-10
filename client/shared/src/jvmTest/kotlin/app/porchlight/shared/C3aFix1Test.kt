package app.porchlight.shared

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.memberColor
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class C3aFix1Test {
    private fun recorded(): ItemRead = WireJson.decodeFromString(javaClass.getResource("/item-fixtures/board.json")!!.readText())
    @Test fun recordedDispositionsAndStatusesPopulateToday() {
        val row = recorded()
        val occurrence = row.occurrence!!
        val rows = listOf(row,
            row.copy(id = "01999999-0000-7000-8000-000000000006", occurrence = occurrence.copy(id = "01999999-0000-7000-8000-000000000010", itemId = "01999999-0000-7000-8000-000000000006", disposition = "overdue")),
            row.copy(id = "01999999-0000-7000-8000-000000000007", occurrence = occurrence.copy(id = "01999999-0000-7000-8000-000000000007", disposition = "expired")),
            row.copy(id = "01999999-0000-7000-8000-000000000008", occurrence = occurrence.copy(id = "01999999-0000-7000-8000-000000000008", status = "skipped")),
            row.copy(id = "01999999-0000-7000-8000-000000000009", occurrence = occurrence.copy(id = "01999999-0000-7000-8000-000000000009", status = "done")))
        val groups = todaySections(rows, TodaySettings(listOf(TodaySection.overdue, TodaySection.today), SettingsScope.household, 7), row.ownerId, TimeZone.UTC, Instant.parse("2026-10-06T12:00:00Z"))
        assertEquals(listOf("01999999-0000-7000-8000-000000000006"), groups[0].rows.map { it.id })
        assertEquals(listOf(row.id), groups[1].rows.map { it.id })
    }
    @Test fun titleOnlyPatchOmitsTimingAndClearIsExplicitNull() {
        val original = EditorDraft.from(recorded().copy(address = "Old address", recurrenceMode = "calendar", rrule = "COUNT=10;FREQ=DAILY"))
        assertEquals(setOf("title"), original.copy(title = "Changed").edit(original).body.keys)
        assertEquals(buildJsonObject { put("address", JsonNull) }, original.copy(address = "").edit(original).body)
        assertTrue(original.edit(original).body.isEmpty())
    }
    @Test fun futureScopeSeedsTailAnchorsAndRemainingCount() {
        val row = recorded().copy(recurrenceMode = "calendar", rrule = "FREQ=DAILY;COUNT=10",
            occurrence = recorded().occurrence!!.copy(seq = 4, dueAt = Instant.parse("2026-10-10T13:00:00Z")))
        val seed = scopeSeed(row, EditorScope.ThisAndFuture)
        assertEquals("2026-10-10T13:00:00Z", seed.dueAt)
        assertEquals("6", seed.count)
        assertEquals(setOf("title"), seed.copy(title = "New title").edit(seed).body.keys)
        val timing = seed.copy(dueAt = "2026-10-10T14:00:00Z").edit(seed).body
        assertEquals(buildJsonObject { put("due_at", "2026-10-10T14:00:00Z") }, timing)
        assertEquals(listOf(EditorScope.ThisOccurrence, EditorScope.All), editorScopes(true, "occ", "completion"))
    }
    @Test fun ambiguousFailureAfterStaleHopKeepsActiveKey() = runTest {
        val writer = ScreenWrites { "old" }
        val keys = mutableListOf<String>()
        var fail = true
        writer.submit(activeKey = { "new" }) { key -> keys += key; if (fail) throw java.io.IOException("transport"); WriteResult.Success(JsonObject(emptyMap())) }
        assertEquals("new", writer.state.value.retryKey)
        fail = false; writer.retry()
        assertEquals(listOf("old", "new"), keys)
    }

    @Test fun displayUsesMemberZoneLocaleAndFloatingWallTime() {
        val old = java.util.Locale.getDefault()
        try {
            java.util.Locale.setDefault(java.util.Locale.US)
            val row = recorded().copy(occurrence = recorded().occurrence!!.copy(dueAt = Instant.parse("2026-10-08T19:30:00Z")))
            val text = itemTimeLabel(row, TimeZone.of("America/New_York"))
            assertTrue(text.contains("3:30"), text)
            assertFalse(text.contains("19:30"), text)
            val floating = row.copy(timeMode = "floating", occurrence = row.occurrence!!.copy(dueAt = null, localDue = "2026-10-08T15:30:00"))
            assertEquals(text, itemTimeLabel(floating, TimeZone.of("America/Los_Angeles")))
            java.util.Locale.setDefault(java.util.Locale.GERMANY)
            val german = itemTimeLabel(row, TimeZone.of("America/New_York"))
            assertTrue(german.contains("15:30"), german)
            assertNotEquals(text, german)
        } finally { java.util.Locale.setDefault(old) }
    }
    @Test fun recurrenceAndColoursUseRealWireValues() {
        assertEquals("Every 2 weeks on Monday, Wednesday · 6 occurrences", recurrenceLabel(recorded().copy(recurrenceMode = "calendar", rrule = "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=6")))
        assertEquals(androidx.compose.ui.graphics.Color(0xFF83A598), memberColor("#83A598", 0))
        assertEquals(androidx.compose.ui.graphics.Color(0xFF123456), memberColor("#123456", 0))
    }
    @Test fun noOccurrenceChangesNeverDetachAndScopeSwitchKeepsChanges() {
        val row = recorded()
        val draft = EditorDraft.from(row).withOccurrence(row.occurrence)
        assertTrue(occurrenceEdit(draft, draft).body.isEmpty())
        val tail = row.copy(recurrenceMode = "calendar", rrule = "FREQ=DAILY;COUNT=10", occurrence = row.occurrence!!.copy(seq = 4, dueAt = Instant.parse("2026-10-10T13:00:00Z")))
        val original = EditorDraft.from(tail)
        val next = scopeSeed(tail, EditorScope.ThisAndFuture)
        val changed = original.copy(title = "Keep this", notes = "Typed notes").rebase(original, next)
        assertEquals("Keep this", changed.title); assertEquals("Typed notes", changed.notes)
        assertEquals("6", changed.count); assertEquals(next.dueAt, changed.dueAt)
    }

    @Test fun localFailureIsNotAnAmbiguousRequest() = runTest {
        val writer = ScreenWrites { "key" }
        writer.submit { throw IllegalArgumentException("invalid draft") }
        assertNull(writer.state.value.retryKey)
        assertFalse(writer.state.value.error!!.contains("confirm"))
    }

    @Test fun recordedAgendaWinsOnlyForCurrentMemberAndOwnerWindowIgnoresMine() {
        val row = recorded()
        val agenda: ItemRead = WireJson.decodeFromString(javaClass.getResource("/item-fixtures/agenda.json")!!.readText())
        val member = agenda.agenda!!.memberId
        val shifted = agenda.copy(agenda = agenda.agenda.copy(dueAt = Instant.parse("2026-10-07T13:00:00Z")))
        val foreign = shifted.copy(title = "Other member", agenda = shifted.agenda!!.copy(memberId = "01999999-0000-7000-8000-000000000009"))
        val view = CacheView(items = listOf(row), occurrences = listOf(row.occurrence!!), agenda = listOf(shifted, foreign, shifted))
        val rows = todayRows(view, member)
        assertEquals(1, rows.size); assertEquals(shifted.agenda, rows.single().agenda)
        val settings = TodaySettings(listOf(TodaySection.today, TodaySection.this_week, TodaySection.needs_owner), SettingsScope.mine, 3)
        val sections = todaySections(rows, settings, member, TimeZone.UTC, Instant.parse("2026-10-06T12:00:00Z"))
        assertTrue(sections[0].rows.isEmpty()); assertEquals(1, sections[1].rows.size)
        val unowned = row.copy(ownerId = foreign.agenda!!.memberId, assignees = emptyList())
        val far = unowned.copy(occurrence = unowned.occurrence!!.copy(id = "01999999-0000-7000-8000-000000000011", dueAt = Instant.parse("2027-10-06T13:00:00Z")))
        val ownerSections = todaySections(listOf(unowned, far), settings, member, TimeZone.UTC, Instant.parse("2026-10-06T12:00:00Z"))
        assertTrue(ownerSections[0].rows.isEmpty())
        assertEquals(listOf(row.occurrence.id), ownerSections[2].rows.map { it.occurrence!!.id })
    }
    @Test fun identityChangeCancelsApplicationWritesScope() {
        val session = ScreenSession()
        session.bind(1)
        val oldScope = session.writeScope
        session.bind(1); assertSame(oldScope, session.writeScope)
        session.bind(2)
        assertFalse(oldScope.coroutineContext[kotlinx.coroutines.Job]!!.isActive)
        assertTrue(session.writeScope.coroutineContext[kotlinx.coroutines.Job]!!.isActive)
    }

    @Test fun comparisonShowsOnlyChangedFieldsWithoutJsonDump() {
        val interval = buildJsonObject { put("months", 0); put("days", 2); put("microseconds", 0) }
        val current = buildJsonObject { put("title", "Current"); put("notes", "Other member notes"); put("completion_interval", interval) }
        val comparison = fieldComparisons(buildJsonObject { put("completion_interval", interval) }, current)
        assertEquals(listOf("Completion interval"), comparison.map { it.label })
        assertFalse(comparison.single().yours.contains('{'))
        assertTrue(comparison.single().yours.contains("Days: 2"))
    }
}
