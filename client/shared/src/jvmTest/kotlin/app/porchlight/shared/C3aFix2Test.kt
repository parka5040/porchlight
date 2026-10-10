package app.porchlight.shared

import app.porchlight.shared.screens.item.*
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.json.*
import kotlin.test.*

class C3aFix2Test {
    private fun checkClear(name: String, original: EditorDraft, changed: EditorDraft, expected: String) {
        // Choosing All must keep every explicit edit, including Kotlin defaults.
        val rebased = changed.rebase(original, original)
        assertEquals(changed, rebased.copy(editedFields = emptySet()), name)
        assertTrue(rebased.editedFields.isNotEmpty(), name)
        assertEquals(WireJson.parseToJsonElement(expected), rebased.edit(original).body, name)
    }
    @Test fun scopeTransitionsResetEveryTimingFieldButPreserveMetadata() {
        val head = EditorDraft(title = "Task", notes = "Old", kind = "appointment", recurrence = "calendar", count = "10",
            startsAt = "2026-10-01T12:00:00Z", endsAt = "2026-10-01T13:00:00Z")
        val tail = head.copy(startsAt = "2026-10-05T12:00:00Z", endsAt = "2026-10-05T13:00:00Z", count = "6")
        val edit = head.copy(title = "Changed", notes = "", startsAt = "2026-10-01T14:00:00Z", endsAt = "2026-10-01T15:00:00Z", count = "9").trackEdits(head)
        val rebased = edit.rebase(head, tail, resetTiming = true)
        assertEquals(tail.startsAt, rebased.startsAt); assertEquals(tail.endsAt, rebased.endsAt); assertEquals("6", rebased.count)
        assertEquals(buildJsonObject { put("title", "Changed"); put("notes", "") }, rebased.edit(tail).body)
        assertTrue(rebased.editedFields.none { it.timing })
        val timed = rebased.copy(startsAt = "2026-10-05T14:00:00Z", endsAt = "2026-10-05T15:00:00Z", count = "5").trackEdits(rebased)
        assertEquals(buildJsonObject { put("title", "Changed"); put("notes", ""); put("starts_at", "2026-10-05T14:00:00Z"); put("ends_at", "2026-10-05T15:00:00Z"); put("rrule", "FREQ=DAILY;COUNT=5") }, timed.edit(tail).body)
        val back = timed.rebase(tail, head, resetTiming = true)
        assertEquals(head.startsAt, back.startsAt); assertEquals(head.endsAt, back.endsAt); assertEquals("10", back.count)
        assertEquals(buildJsonObject { put("title", "Changed"); put("notes", "") }, back.edit(head).body)
        // Exhaustively exercise reset membership, including floating/deadline anchors and schedule settings.
        for (field in EditorField.entries.filter { it.timing }) {
            val reset = edit.copy(editedFields = setOf(field)).rebase(edit, tail, resetTiming = true)
            assertEquals(field.read(tail), field.read(reset), field.name)
            assertTrue(field !in reset.editedFields, field.name)
        }
    }
    @Test fun explicitlyRestoredValueRemainsPresentAcrossSerializationAndReseeding() {
        val seed = EditorDraft(title = "Task", notes = "Old")
        val first = seed.copy(notes = "Typed").trackEdits(seed)
        val restored = first.copy(notes = "Old").trackEdits(first)
        val saved = WireJson.decodeFromString<EditorDraft>(WireJson.encodeToString(restored))
        assertTrue(EditorField.notes in saved.editedFields)
        val next = seed.copy(notes = "Other")
        assertEquals(JsonPrimitive("Old"), saved.rebase(seed, next).edit(next).body["notes"])
    }
    @Test fun defaultValuedEditsSurviveScopeChoiceWithExactBodies() {
        val d = EditorDraft(title = "Task")
        checkClear("notes", d.copy(notes = "Old"), d, """{"notes":""}""")
        checkClear("address", d.copy(address = "Old"), d, """{"address":null}""")
        checkClear("estimate", d.copy(estimate = "30"), d, """{"est_minutes":null}""")
        checkClear("subject", d.copy(subject = "member"), d, """{"subject_member_id":null}""")
        checkClear("priority", d.copy(priority = "3"), d, """{"priority":0}""")
        checkClear("buffer before", d.copy(bufferBefore = "10"), d, """{"buffer_before_min":0}""")
        checkClear("buffer after", d.copy(bufferAfter = "10"), d, """{"buffer_after_min":0}""")
        checkClear("all day", d.copy(allDay = true), d, """{"all_day":false}""")
        checkClear("kind", d.copy(kind = "deadline"), d, """{"kind":"todo"}""")
        checkClear("visibility", d.copy(visibility = "private"), d, """{"visibility":"household"}""")
        checkClear("missed policy", d.copy(missedPolicy = "skip"), d, """{"missed_policy":"carry_over"}""")
        checkClear("time mode", d.copy(timeMode = "floating"), d, """{"time_mode":"fixed","tzid":"UTC"}""")
        checkClear("zone", d.copy(tzid = "Europe/London"), d, """{"tzid":"UTC"}""")
        checkClear("title", d, d.copy(title = ""), """{"title":""}""")
        val calendar = d.copy(recurrence = "calendar")
        checkClear("stop repeating", calendar.copy(count = "10"), d, """{"recurrence_mode":null,"rrule":null}""")
        checkClear("count", calendar.copy(count = "10"), calendar, """{"rrule":"FREQ=DAILY"}""")
        checkClear("until", calendar.copy(until = "20261031T120000Z"), calendar, """{"rrule":"FREQ=DAILY"}""")
        checkClear("by day", calendar.copy(byDay = "MO,WE"), calendar, """{"rrule":"FREQ=DAILY"}""")
        checkClear("by month day", calendar.copy(byMonthDay = "1,15"), calendar, """{"rrule":"FREQ=DAILY"}""")
        checkClear("frequency", calendar.copy(frequency = "WEEKLY"), calendar, """{"rrule":"FREQ=DAILY"}""")
        checkClear("interval", calendar.copy(interval = "2"), calendar, """{"rrule":"FREQ=DAILY"}""")
        val completion = d.copy(recurrence = "completion")
        val interval = """{"completion_interval":{"months":0,"days":1,"microseconds":0}}"""
        checkClear("months", completion.copy(months = "2"), completion, interval)
        checkClear("days", completion.copy(days = "3"), completion, interval)
        checkClear("microseconds", completion.copy(microseconds = "1000"), completion, interval)
        checkClear("due date", d.copy(dueDate = "2026-10-06"), d, """{"due_date":null}""")
        val deadline = d.copy(kind = "deadline")
        checkClear("due instant", deadline.copy(dueAt = "2026-10-06T12:00:00Z"), deadline, """{"due_at":null}""")
        val appointment = d.copy(kind = "appointment")
        checkClear("start instant", appointment.copy(startsAt = "2026-10-06T12:00:00Z"), appointment, """{"starts_at":null}""")
        checkClear("end instant", appointment.copy(endsAt = "2026-10-06T13:00:00Z"), appointment, """{"ends_at":null}""")
        val floating = deadline.copy(timeMode = "floating")
        checkClear("local due", floating.copy(localDue = "2026-10-06T12:00:00"), floating, """{"local_due":null}""")
        val localAppointment = appointment.copy(timeMode = "floating")
        checkClear("local start", localAppointment.copy(localStart = "2026-10-06T12:00:00"), localAppointment, """{"local_start":null}""")
        checkClear("local end", localAppointment.copy(localEnd = "2026-10-06T13:00:00"), localAppointment, """{"local_end":null}""")
        // These draft fields use separate endpoints, never the item PATCH.
        checkClear("assignees", d.copy(assignees = listOf("member")), d, "{}")
        checkClear("viewers", d.copy(viewers = listOf("member")), d, "{}")
        checkClear("reminders", d.copy(reminders = "10"), d, "{}")
    }
}
