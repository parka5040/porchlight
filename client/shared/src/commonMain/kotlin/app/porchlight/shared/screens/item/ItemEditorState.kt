package app.porchlight.shared.screens.item

import androidx.compose.runtime.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.api.models.OccurrenceRead
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.json.*

/** One memory-only editor generation, shared with application-scoped save completion. */
class ItemEditorState(initial: EditorDraft, private val creating: Boolean) {
    var draft by mutableStateOf(initial)
    var baseline by mutableStateOf(initial)
    var editing by mutableStateOf(creating)
    var editScope by mutableStateOf(EditorScope.All)
    var scopeChosen by mutableStateOf(creating)
    var scopeOccurrenceId by mutableStateOf<String?>(null)
    fun hasValidScope(row: app.porchlight.shared.api.models.ItemRead?): Boolean = scopeChosen && row != null &&
        editScope in editorScopes(row.recurrenceMode != null, row.occurrence?.id, row.recurrenceMode) &&
        (editScope == EditorScope.All || scopeOccurrenceId == row.occurrence?.id)
    var expectedVersion by mutableIntStateOf(0)
    var expectedOccurrenceVersion by mutableIntStateOf(0)
    var generation by mutableIntStateOf(0)
    var present by mutableStateOf(false)
    var unsaved by mutableStateOf(false)
    var savedId by mutableStateOf<String?>(null)
    var needsSavedVersion by mutableStateOf(false)
    fun store(value: EditorDraft) { draft = value; present = true; if (creating) unsaved = true; generation++ }
    fun clear() { present = false; editing = false; unsaved = false; needsSavedVersion = false; generation++ }
    fun saved(submitted: Int, frozen: EditorDraft, value: JsonElement, view: CacheView, chosen: EditorScope, submittedBaseline: EditorDraft = baseline) {
        val json = value as? JsonObject
        val id = json?.get("id")?.jsonPrimitive?.contentOrNull
        val version = json?.get("version")?.jsonPrimitive?.intOrNull
        // A scoped endpoint acknowledges only its own fields, not the entire submitted generation.
        val acknowledged = when (chosen) {
            EditorScope.ThisOccurrence -> when (frozen.kind) {
                "todo" -> submittedBaseline.copy(dueDate = frozen.dueDate)
                "deadline" -> if (frozen.timeMode == "fixed") submittedBaseline.copy(dueAt = frozen.dueAt) else submittedBaseline.copy(localDue = frozen.localDue)
                else -> if (frozen.timeMode == "fixed") submittedBaseline.copy(startsAt = frozen.startsAt, endsAt = frozen.endsAt)
                    else submittedBaseline.copy(localStart = frozen.localStart, localEnd = frozen.localEnd)
            }
            EditorScope.ThisAndFuture -> frozen.copy(assignees = submittedBaseline.assignees, viewers = submittedBaseline.viewers, reminders = submittedBaseline.reminders)
            EditorScope.All -> frozen
        }
        val next = if (chosen == EditorScope.ThisOccurrence) {
            if (version != null) expectedOccurrenceVersion = version
            val occurrence = runCatching { WireJson.decodeFromJsonElement<OccurrenceRead>(value) }.getOrNull()
            acknowledged.withOccurrence(occurrence)
        } else {
            if (version != null) expectedVersion = version
            if (id != null) savedId = id
            view.items.find { it.id == id }?.let(EditorDraft::from) ?: acknowledged
        }
        val omitted = EditorField.entries.any { it.read(frozen) != it.read(acknowledged) }
        if ((generation == submitted && !omitted) || !present) clear()
        else {
            needsSavedVersion = version == null || id == null
            editing = true
            // Retain both newer typing and submitted fields omitted by the selected endpoint.
            draft = draft.copy(editedFields = emptySet()).rebase(acknowledged.copy(editedFields = emptySet()), next)
            baseline = next
            unsaved = true
        }
    }
}
