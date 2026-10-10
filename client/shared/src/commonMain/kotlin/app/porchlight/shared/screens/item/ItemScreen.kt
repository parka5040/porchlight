package app.porchlight.shared.screens.item

import kotlinx.datetime.TimeZone

import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
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
    remember(context, creating) {
        // A new visit is a fresh creation, except when a pending ambiguous request must be resumed.
        if (creating && writer.state.value.saved) writer.discard()
        true
    }
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
    val parent = view.items.find { it.id == context } ?: view.board.find { it.id == context } ?: view.agenda.find { it.id == context }
    val occurrence = itemOccurrence(view, context, occurrenceId)
    val row = parent?.copy(occurrence = occurrence)
    var editing by rememberSaveable(context) { mutableStateOf(creating) }
    var editScope by rememberSaveable(context) { mutableStateOf(EditorScope.All) }
    var scopeChosen by rememberSaveable(context) { mutableStateOf(creating) }
    var expectedVersion by rememberSaveable(context) { mutableIntStateOf(0) }
    var expectedOccurrenceVersion by rememberSaveable(context) { mutableIntStateOf(0) }
    var draft by remember(context) { mutableStateOf(services.screenSession?.drafts?.get(context) ?: EditorDraft(tzid = view.members.find { it.id == view.settings?.memberId }?.tzid ?: "UTC")) }
    var baseline by remember(context) { mutableStateOf(services.screenSession?.drafts?.get("$context:baseline") ?: draft) }
    fun seed(value: EditorDraft) { baseline = value; services.screenSession?.drafts?.set("$context:baseline", value) }
    var errors by remember(context) { mutableStateOf<Map<String, String>>(emptyMap()) }
    var deleting by remember { mutableStateOf(false) }
    fun storeDraft(value: EditorDraft) { draft = value; services.screenSession?.drafts?.set(context, value) }
    fun change(value: EditorDraft) { storeDraft(value.trackEdits(draft)) }
    fun beginEdit() {
        if (feedback.retryKey != null || feedback.busy) return
        row?.let { current ->
            expectedVersion = current.version; expectedOccurrenceVersion = current.occurrence?.version ?: 0
            seed(services.screenSession?.drafts?.get("$context:baseline") ?: EditorDraft.from(current))
            storeDraft(services.screenSession?.drafts?.get(context) ?: baseline)
            editScope = EditorScope.All; scopeChosen = current.recurrenceMode == null || current.occurrence == null; editing = true; writer.discard()
        }
    }
    fun save(version: Int = expectedVersion, occurrenceVersion: Int = expectedOccurrenceVersion) {
        if (!creating && row?.recurrenceMode != null && !scopeChosen) { errors = mapOf("scope" to "Choose a recurring edit scope before saving."); return }
        errors = draft.errors()
        if (editScope == EditorScope.ThisOccurrence) errors = errors.filterKeys { it in setOf("due_date", "due_at", "starts_at", "ends_at", "local_due", "local_start", "local_end", "tzid") }
        if (errors.isNotEmpty()) return
        val frozen = draft
        val chosen = editScope
        val item = row
        val member = view.settings?.memberId
        if (creating && member == null) { errors = mapOf("title" to "Wait for initial sync before creating."); return }
        if (!creating && (item == null || "edit" !in item.capabilities)) return
        val edit = if (chosen == EditorScope.ThisOccurrence) occurrenceEdit(frozen, baseline) else ItemEdit(JsonObject(frozen.edit(if (creating) null else baseline).body.filterKeys { field ->
            (field != "visibility" || item == null || "change_visibility" in item.capabilities) &&
                (field != "subject_member_id" || item == null || "reassign" in item.capabilities)
        }))
        if (!creating && edit.body.isEmpty()) { errors = mapOf("scope" to "No changes to save."); return }
        val session = services.screenSession
        val savedBaseline = baseline
        scope.launch { writer.submit { key ->
            when {
                creating -> data.items.create(frozen.create(member!!), key).feedback()
                chosen == EditorScope.ThisOccurrence && item?.occurrence != null -> data.occurrences.patch(item.occurrence.id, occurrenceVersion, edit, key).feedback()
                chosen == EditorScope.ThisAndFuture && item?.occurrence != null -> data.items.edit(context, version, edit, EditScope.ThisAndFuture, item.occurrence.id, key).feedback()
                else -> data.items.edit(context, version, edit, key = key).feedback()
            }.also { if (it is WriteResult.Success) session?.retireSavedDraft(context, savedBaseline) }
        } }
    }
    LaunchedEffect(feedback.saved, editing) {
        if (feedback.saved && editing) {
            editing = false; services.screenSession?.drafts?.remove(context); services.screenSession?.drafts?.remove("$context:baseline")
            if (creating && feedback.warning == null && !feedback.refreshPending) (feedback.savedValue as? JsonObject)?.get("id")?.jsonPrimitive?.contentOrNull?.let {
                navigate(app.porchlight.shared.navigation.AppRoute.Item(it))
            }
        }
    }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
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
            editing = false; services.screenSession?.drafts?.remove(context); services.screenSession?.drafts?.remove("$context:baseline"); errors = emptyMap(); reload++
        })
        if (!creating && row == null) Text("Item unavailable. Refetch or return to Today.")
        else if (editing) {
            if (!creating && row != null && "edit" !in row.capabilities) Text("Editing is no longer permitted.")
            else {
                Row(Modifier.horizontalScroll(rememberScrollState())) {
                    if (!feedback.busy && feedback.retryKey == null) ShellButton("Save item", onClick = { save() })
                    ShellButton("Cancel editor", onClick = { editing = false; if (feedback.retryKey == null) { writer.discard(); services.screenSession?.drafts?.remove(context); services.screenSession?.drafts?.remove("$context:baseline") } })
                }
                EditorForm(draft, ::change, errors + editorFieldErrors(feedback.fieldErrors, editScope, draft.timeMode), view.members, creating, editScope, { chosen ->
                    // A restored draft may be tail-seeded although editScope starts at All.
                    // Its first explicit scope choice must establish a known timing baseline.
                    val resetTiming = !scopeChosen || chosen != editScope
                    editScope = chosen; scopeChosen = true
                    row?.let { current ->
                        val next = scopeSeed(current, chosen)
                        val preserved = draft.rebase(baseline, next, resetTiming)
                        seed(next); storeDraft(preserved); errors = emptyMap()
                    }
                }, Modifier.weight(1f), editorScopes(row?.recurrenceMode != null, row?.occurrence?.id, row?.recurrenceMode), row?.capabilities ?: listOf("edit", "reassign", "change_visibility", "edit_reminders"))
            }
        } else if (creating && feedback.saved) {
            Text("Item created. Review the warnings above before continuing.")
            (feedback.savedValue as? JsonObject)?.get("id")?.jsonPrimitive?.contentOrNull?.let { id ->
                ShellButton("Open created item", onClick = { navigate(app.porchlight.shared.navigation.AppRoute.Item(id)) })
            }
        } else if (row == null) Text("Item unavailable. Refetch or return to Today.")
        else Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(16.dp)) {
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

/**
 * c3a-rr1 N3: runs inside the application-scoped write (also on an identical retry), so a save that completes after
 * the editor left the screen still retires its draft. Only the edit generation this save diffed against is retired:
 * a baseline re-seeded since then belongs to a newer edit and is kept.
 */
internal fun ScreenSession.retireSavedDraft(context: String, savedBaseline: EditorDraft) {
    val stored = drafts["$context:baseline"]
    if (stored == null || stored === savedBaseline) { drafts.remove(context); drafts.remove("$context:baseline") }
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
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
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
