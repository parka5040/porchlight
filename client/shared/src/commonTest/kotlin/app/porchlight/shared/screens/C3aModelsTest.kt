package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class C3aModelsTest {
    private val now = Instant.parse("2026-10-06T01:00:00Z") // still Oct 5 in New York
    private fun card(id: String, date: String?, owner: String = "self") = itemRead(id).copy(ownerId = owner,
        occurrence = occurrenceRead("occ-$id", id).copy(dueDate = date?.let(LocalDate::parse)))
    @Test fun todayUsesMemberDayOrderedSectionsScopeAndWindow() {
        val settings = TodaySettings(listOf(TodaySection.needs_owner, TodaySection.overdue, TodaySection.today, TodaySection.this_week, TodaySection.pinned_list), SettingsScope.mine, 3, "list")
        val rows = listOf(card("old", "2026-10-04"), card("today", "2026-10-05"), card("soon", "2026-10-07"), card("later", "2026-10-08"), card("other", "2026-10-05", "other"))
        val sections = todaySections(rows, settings, "self", TimeZone.of("America/New_York"), now)
        assertEquals(settings.todaySections, sections.map { it.kind })
        assertEquals(listOf("old"), sections[1].rows.map { it.id })
        assertEquals(listOf("today"), sections[2].rows.map { it.id })
        assertEquals(listOf("soon"), sections[3].rows.map { it.id })
        assertEquals("list", sections[4].pinnedListId)
        assertEquals(listOf("other", "today", "soon"), sections[0].rows.map { it.id })
    }
    @Test fun todayKeepsMultipleOpenOccurrencesAndDeduplicatesMemberAgenda() {
        val row = card("a", "2026-10-05")
        val second = row.copy(occurrence = row.occurrence!!.copy(id = "another", dueDate = LocalDate.parse("2026-10-04")))
        val settings = TodaySettings(listOf(TodaySection.overdue, TodaySection.today), SettingsScope.household, 7)
        val sections = todaySections(listOf(row, row, second, row.copy(occurrence = row.occurrence.copy(status = "done"))), settings, "self", TimeZone.of("America/New_York"), now)
        assertEquals(listOf("another"), sections[0].rows.map { it.occurrence!!.id })
        assertEquals(1, sections[1].rows.size)
    }
    @Test fun boardUsesOnlyProjectionSortsByPositionAndDropsWithoutMover() {
        val a = card("a", null).copy(occurrence = occurrenceRead("a", "a", "A"))
        val b = card("b", null).copy(occurrence = occurrenceRead("b", "b", "B"))
        val c = card("c", null).copy(occurrence = occurrenceRead("c", "c", "C"))
        val groups = boardColumns(listOf(c, a, b), null, "self")
        assertEquals(listOf("backlog", "todo", "doing", "done"), groups.map { it.status })
        assertEquals(listOf("a", "b", "c"), groups[1].cards.map { it.id })
        assertEquals(BoardDrop("todo", 0), relativeDrop(groups[1].cards, b, -1))
        assertEquals(BoardDrop("todo", 2), relativeDrop(groups[1].cards, b, 1))
        assertEquals(BoardNeighbours("c", null), boardNeighbours(groups[1].cards, "b", relativeDrop(groups[1].cards, b, 1)))
    }
    @Test fun editorRejectsInvalidShapesAndAsksRecurringScope() {
        assertEquals("Required", EditorDraft(title = " ").errors()["title"])
        assertTrue(EditorDraft(title = "A", kind = "appointment").errors().keys.containsAll(listOf("starts_at", "ends_at")))
        assertTrue(EditorDraft(title = "A", timeMode = "floating", kind = "deadline", localDue = "2026-11-01T01:30:00").errors().isEmpty())
        assertTrue(EditorDraft(title = "A", kind = "deadline", dueAt = "bad").errors().containsKey("due_at"))
        assertTrue(EditorDraft(title = "A", recurrence = "calendar", frequency = "WEEKLY", byDay = "XX").errors().containsKey("byday"))
        assertEquals(listOf(EditorScope.ThisOccurrence, EditorScope.ThisAndFuture, EditorScope.All), editorScopes(true, "occ"))
        assertEquals(listOf(EditorScope.All), editorScopes(false, null))
        val edit = EditorDraft(title = "A", kind = "todo").edit().body
        assertEquals(JsonNull, edit["local_due"])
        assertEquals(JsonNull, edit["rrule"])
    }
    @Test fun feedbackRetainsExactOperationAfterRelockAndNeverAutoReapplies409() = runTest {
        val controller = ScreenWrites { "key" }
        val seen = mutableListOf<String>()
        var locked = true
        controller.submit { key -> seen += key; if (locked) throw HouseholdLockedAfterReply(key); WriteResult.Success(buildJsonObject { put("title", "saved"); put("conflicts", buildJsonArray { add(buildJsonObject { put("other_title", "Busy") }) }) }) }
        assertEquals("key", controller.state.value.retryKey)
        locked = false; controller.retry()
        assertEquals(listOf("key", "key"), seen)
        assertTrue(controller.state.value.warning!!.contains("Busy"))
        val current = buildJsonObject { put("title", "current"); put("version", 3) }
        controller.submit { WriteResult.VersionConflict(current, ApiProblem("version-conflict", 409, "Changed")) }
        assertEquals(current, controller.state.value.current)
        assertNull(controller.state.value.retryKey)
        assertEquals(2, seen.size)
    }
    @Test fun occurrenceEditUsesOverrideFieldsAndFixedItemZone() {
        val draft = EditorDraft(title = "A", kind = "deadline", tzid = "America/New_York", dueAt = "2026-11-01T06:30:00Z")
        assertEquals(setOf("override_local_due"), occurrenceEdit(draft).body.keys)
        assertEquals(JsonPrimitive("2026-11-01T01:30:00"), occurrenceEdit(draft).body["override_local_due"])
        assertEquals(setOf("override_due_date"), occurrenceEdit(EditorDraft(title = "A", dueDate = "2026-10-06")).body.keys)
    }
    @Test fun itemEditDoesNotSendRelationshipOrReminderEndpointFields() {
        val body = EditorDraft(title = "A", assignees = listOf("self"), reminders = "5").edit().body
        assertFalse(body.keys.any { it in setOf("assignees", "viewers", "reminder_offsets") })
    }
    @Test fun cancellationRetainsImmutableKeyAndIdentityEpochClearsScreenSession() = runTest {
        val session = ScreenSession(); session.bind(1)
        val writer = session.writes("item")
        var interrupted = true
        assertFailsWith<kotlinx.coroutines.CancellationException> {
            writer.submit { if (interrupted) throw kotlinx.coroutines.CancellationException("relock"); WriteResult.Success(buildJsonObject {}) }
        }
        assertFalse(writer.state.value.busy)
        assertNotNull(writer.state.value.retryKey)
        assertSame(writer, session.writes("item"))
        interrupted = false; writer.retry(); assertTrue(writer.state.value.saved)
        session.bind(2); assertNotSame(writer, session.writes("item"))
    }
    @Test fun serverOccurrenceErrorsMapToVisibleFixedOrFloatingFields() {
        val wire = mapOf("override_local_due" to "Invalid", "override_due_date" to "Bad date")
        assertEquals(mapOf("due_at" to "Invalid", "due_date" to "Bad date"), editorFieldErrors(wire, EditorScope.ThisOccurrence, "fixed"))
        assertEquals(mapOf("local_due" to "Invalid", "due_date" to "Bad date"), editorFieldErrors(wire, EditorScope.ThisOccurrence, "floating"))
    }
    @Test fun cancellationAfterStaleMoveRetainsActualAttemptKey() = runTest {
        val writer = ScreenWrites { "old-key" }
        assertFailsWith<kotlinx.coroutines.CancellationException> {
            writer.submit(activeKey = { "fresh-key" }) { throw kotlinx.coroutines.CancellationException("relock after fresh move") }
        }
        assertEquals("fresh-key", writer.state.value.retryKey)
    }
    @Test fun createsTypedRequestAndCompletionIntervalAndNormalizesLocalSeconds() {
        val draft = EditorDraft(title = "Meds", recurrence = "completion", dueDate = "2026-10-06", days = "2", assignees = listOf("self"), reminders = "5,5,10")
        val request = draft.create("self")
        assertEquals("self", request.ownerId)
        assertEquals(listOf(5, 10), request.reminderOffsets)
        assertEquals(2, request.completionInterval?.days)
        val local = EditorDraft(title = "A", kind = "deadline", timeMode = "floating", localDue = "2026-10-06T10:00")
        assertEquals(JsonPrimitive("2026-10-06T10:00:00"), local.edit().body["local_due"])
    }
    @Test fun validationErrorsAreMappedToFieldsAndSuccessfulPendingIsNotRetried() = runTest {
        val controller = ScreenWrites { "k" }
        controller.submit { throw ApiProblem("validation-failed", 422, "Invalid", extras = mapOf("errors" to buildJsonArray { add(buildJsonObject { put("field", "title"); put("code", "required") }) })) }
        assertEquals("required", controller.state.value.fieldErrors["title"])
        var calls = 0
        controller.submit { calls++; WriteResult.Success(buildJsonObject {}, true) }
        controller.retry()
        assertEquals(1, calls)
        assertTrue(controller.state.value.refreshPending)
    }
}
