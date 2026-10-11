package app.porchlight.shared.screens.item

import app.porchlight.shared.theme.PorchlightTokens

import kotlinx.datetime.TimeZone

import androidx.compose.runtime.*
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*

@Composable fun ItemScreen(context: String, occurrenceId: String? = null, creating: Boolean = false,
    navigate: (app.porchlight.shared.navigation.AppRoute) -> Unit = {},
    onDeleted: () -> Unit = { navigate(app.porchlight.shared.navigation.AppRoute.Today) },
    onDeletePendingChange: (Boolean) -> Unit = {}, onLoadFinished: () -> Unit = {}, loadKey: Any? = null) {
    val services = LocalScreenServices.current
    val data = services?.data
    if (data == null) { EmptyState("Item $context", "Sign in and sync to load household data."); return }
    val view by data.cache.view.collectAsState()
    val writer = screenWrites("item:$context")
    val feedback by writer.state.collectAsState()
    val scope = applicationWriteScope()
    var loadError by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    val loadFinished by rememberUpdatedState(onLoadFinished)
    val deleted by rememberUpdatedState(onDeleted)
    val deletePendingChange by rememberUpdatedState(onDeletePendingChange)
    // Explicit single-top opens may reuse the same entry and editor, but still need a new fetch.
    LaunchedEffect(context, data, reload, loadKey) {
        if (!creating) {
            try { data.items.get(context); loadError = null }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { loadError = "Could not load this item. It may be offline, deleted or no longer visible." }
            loadFinished()
        }
    }
    val fallbackSession = remember { ScreenSession().also { it.bindCache(data.cache) } }
    val session = services.screenSession ?: fallbackSession
    remember(context, creating, session) {
        // Re-enter New item as a fresh creation, unless a newer draft or pending request remains.
        if (creating && !feedback.busy && feedback.retryKey == null && session.forgetCompletedCreation(context, view)) writer.discard()
        true
    }
    val retainedEditor = session.editor(context, view, creating)
    val editor = checkNotNull(retainedEditor.current(view))
    DisposableEffect(editor) { onDispose { if (!editor.unsaved) editor.editing = false } }
    val itemId = editor.savedId ?: context
    val isCreating = creating && editor.savedId == null
    val parent = view.items.find { it.id == itemId } ?: view.board.find { it.id == itemId } ?: view.agenda.find { it.id == itemId }
    val occurrence = itemOccurrence(view, itemId, occurrenceId)
    val row = parent?.copy(occurrence = occurrence)
    var editing by editor::editing
    var editScope by editor::editScope
    var scopeChosen by editor::scopeChosen
    var expectedVersion by editor::expectedVersion
    var expectedOccurrenceVersion by editor::expectedOccurrenceVersion
    var draft by editor::draft
    var baseline by editor::baseline
    fun seed(value: EditorDraft) { baseline = value }
    var errors by remember(context, view.visibilityRevision) { mutableStateOf<Map<String, String>>(emptyMap()) }
    var deleting by remember { mutableStateOf(false) }
    fun storeDraft(value: EditorDraft) { editor.store(value) }
    fun change(value: EditorDraft) { storeDraft(value.trackEdits(draft)) }
    fun beginEdit() {
        if (feedback.retryKey != null || feedback.busy) return
        row?.let { current ->
            expectedVersion = current.version; expectedOccurrenceVersion = current.occurrence?.version ?: 0
            seed(if (editor.present) baseline else EditorDraft.from(current))
            storeDraft(if (editor.present) draft else baseline)
            editScope = EditorScope.All; scopeChosen = current.recurrenceMode == null
            editor.scopeOccurrenceId = null; editing = true; writer.discard()
        }
    }
    fun save(version: Int = expectedVersion, occurrenceVersion: Int = expectedOccurrenceVersion) {
        if (editor.needsSavedVersion) { errors = mapOf("scope" to "Saved without a version. Keep these newer edits and open the saved item before saving again."); return }
        if (!isCreating && !editor.hasValidScope(row)) { errors = mapOf("scope" to "Choose a recurring edit scope before saving."); return }
        errors = draft.errors()
        if (editScope == EditorScope.ThisOccurrence) errors = errors.filterKeys { it in setOf("due_date", "due_at", "starts_at", "ends_at", "local_due", "local_start", "local_end", "tzid") }
        if (errors.isNotEmpty()) return
        val frozen = draft
        val submittedBaseline = baseline
        val chosen = editScope
        val item = row
        val member = view.settings?.memberId
        if (isCreating && member == null) { errors = mapOf("title" to "Wait for initial sync before creating."); return }
        if (!isCreating && (item == null || "edit" !in item.capabilities)) return
        val edit = if (chosen == EditorScope.ThisOccurrence) occurrenceEdit(frozen, baseline) else ItemEdit(JsonObject(frozen.edit(if (isCreating) null else baseline).body.filterKeys { field ->
            (field != "visibility" || item == null || "change_visibility" in item.capabilities) &&
                (field != "subject_member_id" || item == null || "reassign" in item.capabilities)
        }))
        // A creation may finish while its relationship controls are still being edited. Keep those
        // fields in this editor and commit them through their own versioned endpoints, never PATCH.
        val relationships = creating && !isCreating && chosen == EditorScope.All
        val assigneesChanged = relationships && frozen.assignees != baseline.assignees
        val viewersChanged = relationships && frozen.viewers != baseline.viewers
        val remindersChanged = relationships && frozen.reminders != baseline.reminders
        if (!isCreating && edit.body.isEmpty() && !assigneesChanged && !viewersChanged && !remindersChanged) { errors = mapOf("scope" to "No changes to save."); return }
        val submitted = editor.generation
        var committed = baseline
        var nextVersion = version
        var step = 0
        val steps = buildList {
            if (edit.body.isNotEmpty()) add("item")
            if (assigneesChanged) add("assignees")
            if (viewersChanged) add("viewers")
            if (remindersChanged) add("reminders")
        }
        // ScreenWrites may report the active step's key after relock; it is not a new plan root.
        val submissionKey = randomOperationKey()
        val stepKeys = steps.associateWith { part -> "$submissionKey-$part" }
        fun completed(result: WriteResult<JsonElement>) {
            if (result is WriteResult.Success) retainedEditor.current(data.cache.view.value)?.let { current ->
                current.saved(submitted, frozen, result.value, data.cache.view.value, chosen, submittedBaseline)
                current.savedId?.let { id -> retainedEditor.bindResources(data.cache.view.value, setOf(CacheResource("Item", id))) }
            }
        }
        scope.launch { writer.submit { key ->
            if (relationships && (assigneesChanged || viewersChanged || remindersChanged)) {
                var last: WriteResult<JsonElement> = WriteResult.Success(JsonNull)
                while (step < steps.size) {
                    val current = retainedEditor.current(data.cache.view.value) ?: throw CancellationException("Editor visibility changed")
                    val part = steps[step]
                    val stepKey = stepKeys.getValue(part)
                    val result = when (part) {
                        "assignees" -> data.items.setAssignees(itemId, nextVersion, frozen.assignees, stepKey)
                        "viewers" -> data.items.setViewers(itemId, nextVersion, frozen.viewers, stepKey)
                        "reminders" -> data.items.setReminders(itemId, nextVersion, frozen.reminders.split(',').mapNotNull { it.trim().toIntOrNull() }.distinct(), stepKey)
                        else -> data.items.edit(itemId, nextVersion, edit, key = stepKey)
                    }
                    if (result !is WriteResult.Success) return@submit result.feedback()
                    last = result.feedback()
                    committed = when (part) {
                        "assignees" -> committed.copy(assignees = frozen.assignees)
                        "viewers" -> committed.copy(viewers = frozen.viewers)
                        "reminders" -> committed.copy(reminders = frozen.reminders)
                        else -> frozen.copy(assignees = committed.assignees, viewers = committed.viewers, reminders = committed.reminders)
                    }
                    // A later rejection/retry cannot make already-committed fields unsaved again or
                    // discard fields whose endpoint has not succeeded. Retry resumes this exact step plan.
                    step++
                    current.baseline = committed
                    current.needsSavedVersion = result.value == null
                    current.expectedVersion = result.value?.version ?: nextVersion
                    nextVersion = current.expectedVersion
                    if (result.value == null) { current.unsaved = true; return@submit last }
                }
                completed(last)
                last
            } else when {
                isCreating -> data.items.create(frozen.create(member!!), key).feedback()
                chosen == EditorScope.ThisOccurrence && item?.occurrence != null -> data.occurrences.patch(item.occurrence.id, occurrenceVersion, edit, key).feedback()
                chosen == EditorScope.ThisAndFuture && item?.occurrence != null -> data.items.edit(itemId, version, edit, EditScope.ThisAndFuture, item.occurrence.id, key).feedback()
                else -> data.items.edit(itemId, version, edit, key = key).feedback()
            }.also(::completed)
        } }
    }
    LaunchedEffect(feedback.saved, editing) {
        if (feedback.saved && !editing) {
            if (creating && feedback.warning == null && !feedback.refreshPending) (feedback.savedValue as? JsonObject)?.get("id")?.jsonPrimitive?.contentOrNull?.let {
                navigate(app.porchlight.shared.navigation.AppRoute.Item(it))
            }
        }
    }
    Column(Modifier.fillMaxSize().padding(PorchlightTokens.spaceMd.dp), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        DataStatus(data, loadError, { reload++ })
        ShellButton("Refetch item", onClick = { reload++ })
        if (editing) DesignPending("Item editor and version compare")
        else if (services.platform.kind == app.porchlight.shared.PlatformKind.Desktop) DesignPending("Desktop item detail adaptation")
        WriteStatus(writer, showFieldErrors = !editing, reapply = if ((creating || row != null) && editing && feedback.conflict) ({
            val current = feedback.current as? JsonObject
            val version = current?.get("version")?.jsonPrimitive?.intOrNull
            val caps = current?.get("capabilities") as? JsonArray
            if (version != null && caps?.any { it.jsonPrimitive.content == "edit" } == true) {
                writer.discard()
                if (editScope == EditorScope.ThisOccurrence) { expectedOccurrenceVersion = version; save(occurrenceVersion = version) }
                else { expectedVersion = version; save(version = version) }
            } else { errors = mapOf("title" to "Current is unavailable or cannot be edited. Discard and refetch.") }
        }) else null, comparison = if (editing) fieldComparisons(if (editScope == EditorScope.ThisOccurrence) occurrenceEdit(draft, baseline).body else draft.edit(baseline).body, feedback.current) else emptyList(), onDiscard = {
            editor.clear(); errors = emptyMap(); reload++
        })
        if (!creating && row == null) Text("Item unavailable. Refetch or return to Today.")
        else if (editing) {
            if (!creating && row != null && "edit" !in row.capabilities) Text("Editing is no longer permitted.")
            else {
                if (editor.unsaved) Text("Unsaved changes")
                Row(Modifier.horizontalScroll(rememberScrollState())) {
                    if (!feedback.busy && feedback.retryKey == null) ShellButton("Save item", onClick = { save() })
                    ShellButton("Cancel editor", onClick = { editing = false; if (feedback.retryKey == null) { writer.discard(); editor.clear() } })
                }
                EditorForm(draft, ::change, errors + editorFieldErrors(feedback.fieldErrors, editScope, draft.timeMode), view.members, creating, editScope, { chosen ->
                    // A restored draft may be tail-seeded although editScope starts at All.
                    // Its first explicit scope choice must establish a known timing baseline.
                    val resetTiming = !editor.hasValidScope(row) || chosen != editScope
                    editScope = chosen; scopeChosen = true; editor.scopeOccurrenceId = row?.occurrence?.id
                    row?.let { current ->
                        val next = scopeSeed(current, chosen)
                        expectedOccurrenceVersion = current.occurrence?.version ?: 0
                        val preserved = draft.rebase(baseline, next, resetTiming)
                        seed(next); storeDraft(preserved); errors = emptyMap()
                    }
                }, Modifier.weight(1f), editorScopes(row?.recurrenceMode != null, row?.occurrence?.id, row?.recurrenceMode),
                    row?.capabilities ?: listOf("edit", "reassign", "change_visibility", "edit_reminders"), scopeChosen = isCreating || editor.hasValidScope(row))
            }
        } else if (creating && feedback.saved) {
            Text("Item created. Review the warnings above before continuing.")
            (feedback.savedValue as? JsonObject)?.get("id")?.jsonPrimitive?.contentOrNull?.let { id ->
                ShellButton("Open created item", onClick = { navigate(app.porchlight.shared.navigation.AppRoute.Item(id)) })
            }
        } else if (row == null) Text("Item unavailable. Refetch or return to Today.")
        else Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceMd.dp)) {
            ItemDetailContent(row, view.members, ::beginEdit, {
                row.occurrence?.let { o -> scope.launch { writer.submit { data.occurrences.complete(o.id, o.version, it).feedback() } } }
            }, { status -> row.occurrence?.let { o -> scope.launch { writer.submit { data.occurrences.status(o.id, o.version, OccurrenceStatus.valueOf(status), it).feedback() } } } }, {
                scope.launch { writer.submit { data.items.take(row.id, row.version, it).feedback() } }
            }, { deleting = true })
            RelationshipPanel(row, view.members, data, writer)
            app.porchlight.shared.screens.collaboration.CollaborationSection(row, navigate)
        }
    }
    if (deleting && row != null && "delete" in row.capabilities) AlertDialog(onDismissRequest = { deleting = false }, title = { Text("Delete ${row.title}?") },
        text = { Text("This deletes the whole series, not just this occurrence.") },
        confirmButton = { ShellButton("Confirm delete", onClick = {
            deleting = false
            scope.launch { writer.submit { key ->
                // Eviction is published before repository post-write refresh completes. Keep the
                // local delete distinct from remote eviction, including identical-request retries.
                deletePendingChange(true)
                try {
                    data.items.delete(row.id, row.version, key).feedback().also {
                        if (it is WriteResult.Success) deleted()
                    }
                } finally { deletePendingChange(false) }
            } }
        }) },
        dismissButton = { ShellButton("Keep item", onClick = { deleting = false }) })
}

fun EditorDraft.withOccurrence(o: OccurrenceRead?, resolveFixed: Boolean = true): EditorDraft {
    val effective = o?.effective(if (resolveFixed) timeMode else "floating", runCatching { TimeZone.of(tzid) }.getOrDefault(TimeZone.UTC)) ?: return this
    return copy(dueDate = effective.dueDate?.toString().orEmpty(), dueAt = effective.dueAt?.toString().orEmpty(),
        startsAt = effective.startsAt?.toString().orEmpty(), endsAt = effective.endsAt?.toString().orEmpty(),
        localDue = effective.localDue.orEmpty(), localStart = effective.localStart.orEmpty(), localEnd = effective.localEnd.orEmpty())
}

@Composable fun ItemDetailContent(row: ItemRead, members: List<MemberRead>, edit: () -> Unit, complete: () -> Unit, status: (String) -> Unit, take: () -> Unit, delete: () -> Unit) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        ItemSummary(row, members)
        Text(row.notes)
        Text("Priority ${row.priority} · ${row.estMinutes?.let { "$it minutes" } ?: "No estimate"}")
        Text("Buffers ${row.bufferBeforeMin} / ${row.bufferAfterMin} minutes")
        Text("Visibility: ${row.visibility} · ${if (row.allDay) "All day" else row.kind}")
        Text("Recurrence: ${recurrenceLabel(row)} · ${row.missedPolicy}")
        row.subjectMemberId?.let { id -> Text("Subject: ${members.find { it.id == id }?.displayName ?: "Member"}") }
        row.address?.let { address -> Text(address); ShellButton("Copy address", onClick = { scope.launch { clipboard.setClipEntry(addressClipEntry(address)) } }) }
        if ("edit" in row.capabilities) ShellButton("Edit ${row.title}", onClick = edit)
        if ("complete" in row.capabilities && row.occurrence?.status != "done" && row.occurrence != null) ShellButton("Complete ${row.title}", onClick = complete)
        if ("change_status" in row.capabilities && row.occurrence != null) boardStatuses.forEach { s ->
            ShellButton("Status ${statusLabel(s)}", selected = row.occurrence.status == s, onClick = { status(s) })
        }
        if ("take" in row.capabilities) ShellButton("Take ${row.title}", onClick = take)
        if ("delete" in row.capabilities) ShellButton("Delete ${row.title}", onClick = delete)
    }
}

@Composable private fun RelationshipPanel(row: ItemRead, members: List<MemberRead>, data: HouseholdData, writer: ScreenWrites) {
    val scope = applicationWriteScope()
    var assignees by remember(row.id, row.version) { mutableStateOf(row.assignees) }
    var viewers by remember(row.id, row.version) { mutableStateOf(row.viewers) }
    val view by data.cache.view.collectAsState()
    val provisional = view.itemWrites[row.id]?.reminderOffsets
    var offsets by remember(row.id, row.version, provisional) { mutableStateOf(provisional?.joinToString(",") ?: row.reminders.joinToString(",") { it.offsetMinutes.toString() }) }
    var error by remember { mutableStateOf<String?>(null) }
    if ("reassign" in row.capabilities) {
        MemberChoices("Assignees (whole series)", members, assignees) { assignees = it }
        ShellButton("Save assignees", onClick = { val frozen = assignees; scope.launch { writer.submit { data.items.setAssignees(row.id, row.version, frozen, it).feedback() } } })
    }
    if ("change_visibility" in row.capabilities) {
        MemberChoices("Viewers (whole series)", members, viewers) { viewers = it }
        ShellButton("Save viewers", onClick = { val frozen = viewers; scope.launch { writer.submit { data.items.setViewers(row.id, row.version, frozen, it).feedback() } } })
    }
    if ("edit_reminders" in row.capabilities) {
        FormField("Reminder offsets (whole series)", "reminder_offsets", offsets, { offsets = it }, error?.let { mapOf("reminder_offsets" to it) }.orEmpty())
        ShellButton("Save reminders", onClick = {
            val fields = offsets.takeIf { it.isNotBlank() }?.split(',').orEmpty()
            val frozen = fields.mapNotNull { it.trim().toIntOrNull() }.distinct()
            if (fields.any { it.trim().toIntOrNull() == null }) error = "Enter comma-separated integer minutes"
            else { error = null; scope.launch { writer.submit { data.items.setReminders(row.id, row.version, frozen, it).feedback() } } }
        })
    }
}
