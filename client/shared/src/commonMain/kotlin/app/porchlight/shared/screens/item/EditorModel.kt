package app.porchlight.shared.screens.item

import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.data.ItemEdit
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.time.Instant

enum class EditorScope(val label: String) { ThisOccurrence("This occurrence"), ThisAndFuture("This and future"), All("All occurrences") }
fun editorScopes(recurring: Boolean, occurrenceId: String?, recurrenceMode: String? = "calendar") = if (recurring && occurrenceId != null) EditorScope.entries.filter { it != EditorScope.ThisAndFuture || recurrenceMode == "calendar" } else listOf(EditorScope.All)
fun editorFieldErrors(errors: Map<String, String>, scope: EditorScope, timeMode: String): Map<String, String> =
    if (scope != EditorScope.ThisOccurrence) errors else errors.mapKeys { (field, _) -> when (field) {
        "override_due_date" -> "due_date"
        "override_local_due" -> if (timeMode == "fixed") "due_at" else "local_due"
        "override_local_start" -> if (timeMode == "fixed") "starts_at" else "local_start"
        "override_local_end" -> if (timeMode == "fixed") "ends_at" else "local_end"
        else -> field
    } }

/** Explicit form-field presence; independent of JSON default/omission rules. */
@kotlinx.serialization.Serializable
enum class EditorField(val timing: Boolean, val read: (EditorDraft) -> Any) {
    title(false, { it.title }),
    notes(false, { it.notes }),
    kind(true, { it.kind }),
    timeMode(true, { it.timeMode }),
    tzid(true, { it.tzid }),
    dueDate(true, { it.dueDate }),
    dueAt(true, { it.dueAt }),
    startsAt(true, { it.startsAt }),
    endsAt(true, { it.endsAt }),
    localDue(true, { it.localDue }),
    localStart(true, { it.localStart }),
    localEnd(true, { it.localEnd }),
    allDay(true, { it.allDay }),
    recurrence(false, { it.recurrence }),
    frequency(true, { it.frequency }),
    interval(true, { it.interval }),
    byDay(true, { it.byDay }),
    byMonthDay(true, { it.byMonthDay }),
    count(true, { it.count }),
    until(true, { it.until }),
    months(true, { it.months }),
    days(true, { it.days }),
    microseconds(true, { it.microseconds }),
    missedPolicy(false, { it.missedPolicy }),
    bufferBefore(false, { it.bufferBefore }),
    bufferAfter(false, { it.bufferAfter }),
    estimate(false, { it.estimate }),
    priority(false, { it.priority }),
    address(false, { it.address }),
    assignees(false, { it.assignees }),
    viewers(false, { it.viewers }),
    subject(false, { it.subject }),
    visibility(false, { it.visibility }),
    reminders(false, { it.reminders })
}

/** Form state is not a wire DTO. Generated read models seed it; ItemEdit preserves explicit clears. */
@kotlinx.serialization.Serializable
data class EditorDraft(
    val title: String = "", val notes: String = "", val kind: String = "todo", val timeMode: String = "fixed", val tzid: String = "UTC",
    val dueDate: String = "", val dueAt: String = "", val startsAt: String = "", val endsAt: String = "",
    val localDue: String = "", val localStart: String = "", val localEnd: String = "", val allDay: Boolean = false,
    val recurrence: String = "none", val frequency: String = "DAILY", val interval: String = "1", val byDay: String = "", val byMonthDay: String = "",
    val count: String = "", val until: String = "", val months: String = "0", val days: String = "1", val microseconds: String = "0",
    val missedPolicy: String = "carry_over", val bufferBefore: String = "0", val bufferAfter: String = "0", val estimate: String = "", val priority: String = "0",
    val address: String = "", val assignees: List<String> = emptyList(), val viewers: List<String> = emptyList(), val subject: String = "",
    val visibility: String = "household", val reminders: String = "",
    val editedFields: Set<EditorField> = emptySet(),
) {
    fun rule(): String = buildList {
        add("FREQ=$frequency"); if (interval != "1") add("INTERVAL=$interval")
        if (byDay.isNotBlank()) add("BYDAY=${byDay.uppercase().replace(" ", "")}")
        if (byMonthDay.isNotBlank()) add("BYMONTHDAY=${byMonthDay.replace(" ", "")}")
        if (count.isNotBlank()) add("COUNT=$count")
        if (until.isNotBlank()) add("UNTIL=$until")
    }.joinToString(";")
    fun errors(): Map<String, String> = buildMap {
        if (title.isBlank()) put("title", "Required")
        fun integer(field: String, value: String, min: Int, max: Int, optional: Boolean = false) {
            if (optional && value.isBlank()) return
            if (value.toIntOrNull()?.let { it in min..max } != true) put(field, "Enter $min–$max")
        }
        integer("priority", priority, Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())
        integer("buffer_before_min", bufferBefore, 0, 10080); integer("buffer_after_min", bufferAfter, 0, 10080)
        integer("est_minutes", estimate, 0, Int.MAX_VALUE, true)
        if (dueDate.isNotBlank() && runCatching { LocalDate.parse(dueDate) }.isFailure) put("due_date", "Use YYYY-MM-DD")
        if (timeMode == "fixed") {
            if (runCatching { TimeZone.of(tzid) }.isFailure) put("tzid", "Choose an IANA time zone")
            fun instant(field: String, value: String, required: Boolean) {
                if (value.isBlank()) { if (required) put(field, "Required") }
                else if (runCatching { Instant.parse(value) }.isFailure) put(field, "Use an RFC3339 instant with offset")
            }
            instant("due_at", dueAt, kind == "deadline"); instant("starts_at", startsAt, kind == "appointment"); instant("ends_at", endsAt, kind == "appointment")
            if (kind == "appointment" && runCatching { Instant.parse(endsAt) <= Instant.parse(startsAt) }.getOrDefault(false)) put("ends_at", "Must be after start")
        } else {
            fun local(field: String, value: String, required: Boolean) {
                if (value.isBlank()) { if (required) put(field, "Required") }
                else if (runCatching { LocalDateTime.parse(value) }.isFailure) put(field, "Use YYYY-MM-DDTHH:MM:SS without offset")
            }
            local("local_due", localDue, kind == "deadline"); local("local_start", localStart, kind == "appointment"); local("local_end", localEnd, kind == "appointment")
            if (kind == "appointment" && runCatching { LocalDateTime.parse(localEnd) <= LocalDateTime.parse(localStart) }.getOrDefault(false)) put("local_end", "Must be after start")
        }
        if (allDay && kind != "appointment") put("all_day", "All-day is for appointments")
        if (recurrence == "calendar") {
            integer("interval", interval, 1, Int.MAX_VALUE)
            integer("count", count, 1, Int.MAX_VALUE, true)
            if (count.isNotBlank() && until.isNotBlank()) put("until", "Choose COUNT or UNTIL, not both")
            if (until.isNotBlank() && !Regex("[0-9]{8}(T[0-9]{6}Z)?").matches(until)) put("until", "Use YYYYMMDD or YYYYMMDDTHHMMSSZ")
            if (byDay.isNotBlank() && byDay.uppercase().replace(" ", "").split(',').any { !Regex("([+-]?[1-5])?(MO|TU|WE|TH|FR|SA|SU)").matches(it) }) put("byday", "Use MO,TU,…; monthly ordinal e.g. 1MO")
            if (byMonthDay.isNotBlank() && byMonthDay.split(',').any { it.trim().toIntOrNull()?.let { n -> n in -31..31 && n != 0 } != true }) put("bymonthday", "Use nonzero days -31…31")
            if (kind == "todo" && dueDate.isBlank()) put("due_date", "Recurring todos need a date anchor")
        }
        if (recurrence == "completion") {
            integer("months", months, 0, Int.MAX_VALUE); integer("days", days, 0, Int.MAX_VALUE)
            if (microseconds.toLongOrNull()?.let { it >= 0 } != true) put("microseconds", "Enter a nonnegative integer")
            if (months.toIntOrNull() == 0 && days.toIntOrNull() == 0 && microseconds.toLongOrNull() == 0L) put("completion_interval", "Interval must advance time")
            if (kind == "todo" && dueDate.isBlank()) put("due_date", "Recurring todos need a date anchor")
        }
        if (reminders.isNotBlank() && reminders.split(',').any { it.trim().toIntOrNull() == null }) put("reminder_offsets", "Enter comma-separated minutes")
    }
    private fun fullEdit(): JsonObject = buildJsonObject {
        put("title", title.trim()); put("notes", notes); put("kind", kind); put("time_mode", timeMode)
        put("all_day", allDay); put("priority", priority.toInt()); put("buffer_before_min", bufferBefore.toInt()); put("buffer_after_min", bufferAfter.toInt())
        fun nullable(field: String, value: String) {
            val wire = if (field.startsWith("local_") && value.length == 16) "$value:00" else value
            put(field, if (wire.isBlank()) JsonNull else JsonPrimitive(wire))
        }
        nullable("tzid", if (timeMode == "fixed") tzid else "")
        nullable("due_date", if (kind == "todo") dueDate else "")
        nullable("due_at", if (kind == "deadline" && timeMode == "fixed") dueAt else "")
        nullable("starts_at", if (kind == "appointment" && timeMode == "fixed") startsAt else "")
        nullable("ends_at", if (kind == "appointment" && timeMode == "fixed") endsAt else "")
        nullable("local_due", if (kind == "deadline" && timeMode == "floating") localDue else "")
        nullable("local_start", if (kind == "appointment" && timeMode == "floating") localStart else "")
        nullable("local_end", if (kind == "appointment" && timeMode == "floating") localEnd else "")
        nullable("address", address); nullable("subject_member_id", subject)
        put("est_minutes", estimate.toIntOrNull()?.let(::JsonPrimitive) ?: JsonNull)
        put("visibility", visibility); put("missed_policy", missedPolicy)
        nullable("recurrence_mode", if (recurrence == "none") "" else recurrence)
        nullable("rrule", if (recurrence == "calendar") rule() else "")
        put("completion_interval", if (recurrence == "completion") buildJsonObject { put("months", months.toInt()); put("days", days.toInt()); put("microseconds", microseconds.toLong()) } else JsonNull)
        // Relationships and reminders have their own versioned endpoints; not legal item PATCH fields.
    }
    fun edit(original: EditorDraft? = null): ItemEdit {
        val full = fullEdit()
        val before = original?.fullEdit()
        return ItemEdit(JsonObject(full.filter { (key, value) -> before == null || before[key] != value }))
    }
    companion object {
        fun from(item: ItemRead): EditorDraft {
            val rule = item.rrule.orEmpty().removePrefix("RRULE:").split(';').mapNotNull { p -> p.split('=', limit = 2).takeIf { it.size == 2 }?.let { it[0] to it[1] } }.toMap()
            return EditorDraft(item.title, item.notes, item.kind, item.timeMode, item.tzid ?: "UTC", item.dueDate?.toString().orEmpty(), item.dueAt?.toString().orEmpty(),
                item.startsAt?.toString().orEmpty(), item.endsAt?.toString().orEmpty(), item.localDue.orEmpty(), item.localStart.orEmpty(), item.localEnd.orEmpty(), item.allDay,
                item.recurrenceMode ?: "none", rule["FREQ"] ?: "DAILY", rule["INTERVAL"] ?: "1", rule["BYDAY"].orEmpty(), rule["BYMONTHDAY"].orEmpty(), rule["COUNT"].orEmpty(), rule["UNTIL"].orEmpty(),
                item.completionInterval?.months?.toString() ?: "0", item.completionInterval?.days?.toString() ?: "1", item.completionInterval?.microseconds?.toString() ?: "0", item.missedPolicy,
                item.bufferBeforeMin.toString(), item.bufferAfterMin.toString(), item.estMinutes?.toString().orEmpty(), item.priority.toString(), item.address.orEmpty(), item.assignees, item.viewers, item.subjectMemberId.orEmpty(), item.visibility,
                item.reminders.joinToString(",") { it.offsetMinutes.toString() })
        }
    }
}

/** This occurrence only supports anchor overrides; series metadata and relationships must not leak into PATCH. */
fun occurrenceEdit(draft: EditorDraft): ItemEdit = ItemEdit(buildJsonObject {
    fun anchor(name: String, fixed: String, floating: String) {
        val value = if (draft.timeMode == "fixed") Instant.parse(fixed).toLocalDateTime(TimeZone.of(draft.tzid)).toString() else LocalDateTime.parse(floating).toString()
        // The API's local-date-time format always includes seconds.
        put("override_local_$name", if (value.length == 16) "$value:00" else value)
    }
    when (draft.kind) {
        "todo" -> put("override_due_date", draft.dueDate.takeIf { it.isNotBlank() }?.let(::JsonPrimitive) ?: JsonNull)
        "deadline" -> anchor("due", draft.dueAt, draft.localDue)
        "appointment" -> { anchor("start", draft.startsAt, draft.localStart); anchor("end", draft.endsAt, draft.localEnd) }
    }
})

fun EditorDraft.create(memberId: String): app.porchlight.shared.api.models.CreateRequest {
    val body = JsonObject(edit().body + mapOf(
        "owner_id" to JsonPrimitive(memberId), "assignees" to JsonArray(assignees.map(::JsonPrimitive)),
        "viewers" to JsonArray(viewers.map(::JsonPrimitive)), "reminder_offsets" to JsonArray(reminders.split(',').mapNotNull { it.trim().toIntOrNull() }.distinct().map(::JsonPrimitive))))
    return app.porchlight.shared.transport.WireJson.decodeFromJsonElement(body)
}

fun scopeSeed(item: ItemRead, scope: EditorScope): EditorDraft {
    val draft = EditorDraft.from(item)
    val o = item.occurrence ?: return draft
    return when (scope) {
        EditorScope.All -> draft
        EditorScope.ThisOccurrence -> draft.withOccurrence(o, resolveFixed = item.tzid != null)
        EditorScope.ThisAndFuture -> draft.copy(
            dueDate = o.dueDate?.toString().orEmpty(), dueAt = o.dueAt?.toString().orEmpty(),
            startsAt = o.startsAt?.toString().orEmpty(), endsAt = o.endsAt?.toString().orEmpty(),
            localDue = o.localDue.orEmpty(), localStart = o.localStart.orEmpty(), localEnd = o.localEnd.orEmpty(),
            count = draft.count.toIntOrNull()?.let { (it - o.seq).coerceAtLeast(1).toString() }.orEmpty())
    }
}
/** Called on user input: even a change to a default (or back to the seed) remains explicit. */
fun EditorDraft.trackEdits(previous: EditorDraft): EditorDraft = copy(
    editedFields = editedFields + previous.editedFields + EditorField.entries.filter { it.read(this) != it.read(previous) })

/** A different scope resets anchors/schedule to its seed; metadata edits remain present. */
fun EditorDraft.rebase(original: EditorDraft, current: EditorDraft, resetTiming: Boolean = false): EditorDraft {
    val retained = trackEdits(original).editedFields.filterNot { resetTiming && it.timing }.toSet()
    fun <T> keep(field: EditorField, value: T, seed: T): T = if (field in retained) value else seed
    return current.copy(
        title = keep(EditorField.title, title, current.title),
        notes = keep(EditorField.notes, notes, current.notes),
        kind = keep(EditorField.kind, kind, current.kind),
        timeMode = keep(EditorField.timeMode, timeMode, current.timeMode),
        tzid = keep(EditorField.tzid, tzid, current.tzid),
        dueDate = keep(EditorField.dueDate, dueDate, current.dueDate),
        dueAt = keep(EditorField.dueAt, dueAt, current.dueAt),
        startsAt = keep(EditorField.startsAt, startsAt, current.startsAt),
        endsAt = keep(EditorField.endsAt, endsAt, current.endsAt),
        localDue = keep(EditorField.localDue, localDue, current.localDue),
        localStart = keep(EditorField.localStart, localStart, current.localStart),
        localEnd = keep(EditorField.localEnd, localEnd, current.localEnd),
        allDay = keep(EditorField.allDay, allDay, current.allDay),
        recurrence = keep(EditorField.recurrence, recurrence, current.recurrence),
        frequency = keep(EditorField.frequency, frequency, current.frequency),
        interval = keep(EditorField.interval, interval, current.interval),
        byDay = keep(EditorField.byDay, byDay, current.byDay),
        byMonthDay = keep(EditorField.byMonthDay, byMonthDay, current.byMonthDay),
        count = keep(EditorField.count, count, current.count),
        until = keep(EditorField.until, until, current.until),
        months = keep(EditorField.months, months, current.months),
        days = keep(EditorField.days, days, current.days),
        microseconds = keep(EditorField.microseconds, microseconds, current.microseconds),
        missedPolicy = keep(EditorField.missedPolicy, missedPolicy, current.missedPolicy),
        bufferBefore = keep(EditorField.bufferBefore, bufferBefore, current.bufferBefore),
        bufferAfter = keep(EditorField.bufferAfter, bufferAfter, current.bufferAfter),
        estimate = keep(EditorField.estimate, estimate, current.estimate),
        priority = keep(EditorField.priority, priority, current.priority),
        address = keep(EditorField.address, address, current.address),
        assignees = keep(EditorField.assignees, assignees, current.assignees),
        viewers = keep(EditorField.viewers, viewers, current.viewers),
        subject = keep(EditorField.subject, subject, current.subject),
        visibility = keep(EditorField.visibility, visibility, current.visibility),
        reminders = keep(EditorField.reminders, reminders, current.reminders),
        editedFields = retained)
}
fun occurrenceEdit(draft: EditorDraft, original: EditorDraft): ItemEdit {
    val before = occurrenceEdit(original).body
    return ItemEdit(JsonObject(occurrenceEdit(draft).body.filter { (key, value) -> before[key] != value }))
}
