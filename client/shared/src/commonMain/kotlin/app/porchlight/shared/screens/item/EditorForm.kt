package app.porchlight.shared.screens.item

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.ui.ShellButton
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.screens.fieldLabel

@Composable fun FormField(label: String, field: String, value: String, change: (String) -> Unit, errors: Map<String, String> = emptyMap()) {
    OutlinedTextField(value, change, colors = OutlinedTextFieldDefaults.colors(errorLabelColor = LocalPorchlightPalette.current.errorText, errorSupportingTextColor = LocalPorchlightPalette.current.errorText), label = { Text(label) }, isError = field in errors,
        supportingText = { errors[field]?.let { Text(it, color = LocalPorchlightPalette.current.errorText) } },
        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).semantics { contentDescription = label })
}
@Composable fun Choices(label: String, choices: List<String>, selected: String, change: (String) -> Unit) {
    Text(label, style = MaterialTheme.typography.titleSmall)
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
        choices.forEach { value -> ShellButton(value, "$label: $value", selected = value == selected, onClick = { change(value) }) }
    }
}
@Composable fun MemberChoices(label: String, members: List<MemberRead>, chosen: List<String>, change: (List<String>) -> Unit) {
    Text(label, style = MaterialTheme.typography.titleSmall)
    Column { members.forEach { m -> ShellButton(m.displayName, "$label: ${m.displayName}", selected = m.id in chosen,
        onClick = { change(if (m.id in chosen) chosen - m.id else chosen + m.id) }) } }
}
/**
 * Wire keys of the text controls [EditorForm] renders for this draft/scope: the single source for both rendering and
 * error placement. A field error whose key has no rendered control (occurrence_id, override, assignees, a timing
 * field of another kind, ...) is listed at the top of the form instead of being swallowed (c3a-rr1 N1).
 */
fun editorTextControls(draft: EditorDraft, creating: Boolean, scope: EditorScope): Set<String> = buildSet {
    val occurrenceOnly = scope == EditorScope.ThisOccurrence
    val fixed = draft.timeMode == "fixed"
    if (!occurrenceOnly) { add("title"); add("notes"); if (fixed) add("tzid") }
    when (draft.kind) {
        "todo" -> add("due_date")
        "deadline" -> add(if (fixed) "due_at" else "local_due")
        "appointment" -> if (fixed) { add("starts_at"); add("ends_at") } else { add("local_start"); add("local_end") }
    }
    if (!occurrenceOnly) {
        if (draft.recurrence == "calendar") addAll(listOf("interval", "byday", "bymonthday", "count", "until"))
        if (draft.recurrence == "completion") addAll(listOf("months", "days", "microseconds"))
        addAll(listOf("buffer_before_min", "buffer_after_min", "est_minutes", "priority", "address"))
        if (creating) add("reminder_offsets")
    }
}
@Composable fun EditorForm(draft: EditorDraft, change: (EditorDraft) -> Unit, errors: Map<String, String>, members: List<MemberRead>,
    creating: Boolean, scope: EditorScope, changeScope: (EditorScope) -> Unit, modifier: Modifier = Modifier,
    scopes: List<EditorScope> = listOf(EditorScope.All), capabilities: List<String> = listOf("edit", "reassign", "change_visibility", "edit_reminders")) {
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (scopes.size > 1) {
            Text("Choose recurring edit scope before adjusting timing. Changing scope resets timing and recurrence schedule fields to that scope; other edits are kept.")
            scopes.forEach { s -> ShellButton(s.label, "Edit scope: ${s.label}", selected = scope == s, onClick = { changeScope(s) }) }
        }
        val controls = editorTextControls(draft, creating, scope)
        errors.filterKeys { it !in controls }.forEach { (field, message) -> Text(if (field == "scope") message else "${fieldLabel(field)}: $message", color = LocalPorchlightPalette.current.errorText) }
        val occurrenceOnly = scope == EditorScope.ThisOccurrence
        if (!occurrenceOnly) {
            if ("title" in controls) FormField("Title", "title", draft.title, { change(draft.copy(title = it)) }, errors)
            if ("notes" in controls) FormField("Notes", "notes", draft.notes, { change(draft.copy(notes = it)) }, errors)
            Choices("Kind", listOf("todo", "deadline", "appointment"), draft.kind) { change(draft.copy(kind = it)) }
            Choices("Time mode", listOf("fixed", "floating"), draft.timeMode) { change(draft.copy(timeMode = it)) }
        } else {
            Text("Only this occurrence's anchors change. Series metadata stays unchanged.")
            if (draft.timeMode == "fixed") Text("Overrides use the item's zone (${draft.tzid}); DST folds use the earlier offset and gaps the next valid wall time.")
        }
        if ("tzid" in controls) {
            FormField("Time zone", "tzid", draft.tzid, { change(draft.copy(tzid = it)) }, errors)
            Choices("Common IANA zones", listOf("UTC", "America/New_York", "America/Chicago", "America/Denver", "America/Los_Angeles", "Europe/London"), draft.tzid) { change(draft.copy(tzid = it)) }
        }
        Text(if (draft.timeMode == "fixed") "Fixed times use an ISO instant with offset (e.g. 2026-10-06T14:00:00-04:00)." else "Floating times use local YYYY-MM-DDTHH:MM:SS.")
        if ("due_date" in controls) FormField("Due date", "due_date", draft.dueDate, { change(draft.copy(dueDate = it)) }, errors)
        if ("due_at" in controls) FormField("Due instant", "due_at", draft.dueAt, { change(draft.copy(dueAt = it)) }, errors)
        if ("local_due" in controls) FormField("Local due time", "local_due", draft.localDue, { change(draft.copy(localDue = it)) }, errors)
        if (draft.kind == "appointment") {
            if ("starts_at" in controls) FormField("Start instant", "starts_at", draft.startsAt, { change(draft.copy(startsAt = it)) }, errors)
            if ("ends_at" in controls) FormField("End instant", "ends_at", draft.endsAt, { change(draft.copy(endsAt = it)) }, errors)
            if ("local_start" in controls) FormField("Local start", "local_start", draft.localStart, { change(draft.copy(localStart = it)) }, errors)
            if ("local_end" in controls) FormField("Local end", "local_end", draft.localEnd, { change(draft.copy(localEnd = it)) }, errors)
            if (!occurrenceOnly) ShellButton("All day", selected = draft.allDay, onClick = { change(draft.copy(allDay = !draft.allDay)) })
        }
        if (!occurrenceOnly) {
            Choices("Recurrence", listOf("none", "calendar", "completion"), draft.recurrence) { change(draft.copy(recurrence = it)) }
            if (draft.recurrence == "calendar") {
                Choices("Frequency", listOf("DAILY", "WEEKLY", "MONTHLY", "YEARLY"), draft.frequency) { change(draft.copy(frequency = it)) }
                if ("interval" in controls) FormField("Repeat every", "interval", draft.interval, { change(draft.copy(interval = it)) }, errors)
                if ("byday" in controls) FormField("Days (MO,TU,…; ordinal e.g. 1MO)", "byday", draft.byDay, { change(draft.copy(byDay = it)) }, errors)
                if ("bymonthday" in controls) FormField("Month days (1…31, -1 for last)", "bymonthday", draft.byMonthDay, { change(draft.copy(byMonthDay = it)) }, errors)
                if ("count" in controls) FormField("Count", "count", draft.count, { change(draft.copy(count = it)) }, errors)
                if ("until" in controls) FormField("Until (YYYYMMDD or YYYYMMDDTHHMMSSZ)", "until", draft.until, { change(draft.copy(until = it)) }, errors)
                Text("Rule: ${draft.rule()}")
            }
            if (draft.recurrence == "completion") {
                if ("months" in controls) FormField("Interval months", "months", draft.months, { change(draft.copy(months = it)) }, errors)
                if ("days" in controls) FormField("Interval days", "days", draft.days, { change(draft.copy(days = it)) }, errors)
                if ("microseconds" in controls) FormField("Interval microseconds", "microseconds", draft.microseconds, { change(draft.copy(microseconds = it)) }, errors)
            }
            Choices("Missed policy", listOf("carry_over", "skip", "stack"), draft.missedPolicy) { change(draft.copy(missedPolicy = it)) }
            if ("buffer_before_min" in controls) FormField("Buffer before (minutes)", "buffer_before_min", draft.bufferBefore, { change(draft.copy(bufferBefore = it)) }, errors)
            if ("buffer_after_min" in controls) FormField("Buffer after (minutes)", "buffer_after_min", draft.bufferAfter, { change(draft.copy(bufferAfter = it)) }, errors)
            if ("est_minutes" in controls) FormField("Estimate (minutes)", "est_minutes", draft.estimate, { change(draft.copy(estimate = it)) }, errors)
            if ("priority" in controls) FormField("Priority", "priority", draft.priority, { change(draft.copy(priority = it)) }, errors)
            if ("address" in controls) FormField("Address", "address", draft.address, { change(draft.copy(address = it)) }, errors)
            if ("change_visibility" in capabilities) Choices("Visibility", listOf("household", "private", "selected"), draft.visibility) { change(draft.copy(visibility = it)) }
            if (creating) {
                MemberChoices("Assignees", members, draft.assignees) { change(draft.copy(assignees = it)) }
                MemberChoices("Viewers", members, draft.viewers) { change(draft.copy(viewers = it)) }
            }
            if ("reassign" in capabilities) {
                Text("Subject")
                ShellButton("No subject", selected = draft.subject.isEmpty(), onClick = { change(draft.copy(subject = "")) })
                members.forEach { m -> ShellButton(m.displayName, "Subject: ${m.displayName}", selected = draft.subject == m.id, onClick = { change(draft.copy(subject = m.id)) }) }
            }
            if ("reminder_offsets" in controls) FormField("Reminder offsets (minutes)", "reminder_offsets", draft.reminders, { change(draft.copy(reminders = it)) }, errors)
        }
    }
}
