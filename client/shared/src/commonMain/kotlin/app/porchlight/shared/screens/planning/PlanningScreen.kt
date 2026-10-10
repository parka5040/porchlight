package app.porchlight.shared.screens.planning

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import app.porchlight.shared.AppServices
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.files.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.ui.ShellButton
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.time.Duration.Companion.seconds

class PendingUiExport(val sink: ExportSink, val token: String?) {
    val result = CompletableDeferred<Unit>()
    private val started = CompletableDeferred<Unit>()
    private val deadline = kotlin.time.TimeSource.Monotonic.markNow() + 60.seconds
    fun begin(): Boolean {
        if (deadline.hasPassedNow()) cancel()
        return !result.isCompleted && started.complete(Unit)
    }
    fun cancel() {
        val failure = CancellationException("Export interrupted before completion. Try again.")
        started.completeExceptionally(failure)
        result.completeExceptionally(failure)
    }
    suspend fun awaitCompletion() {
        try {
            // Bound only the unlock wait, not a running streaming download.
            withTimeout(60_000) { started.await() }
            result.await()
        } catch (e: Exception) { cancel(); throw e }
    }
}
fun ScreenSession.cancelOrphanedPlanningExports(activeId: String) {
    planningDrafts.filterKeys { it != activeId }.values.forEach { it.export?.cancel() }
}
class PlanningDraft {
    var fileBusy by mutableStateOf(false)
    var notice by mutableStateOf<String?>(null)
    var fileError by mutableStateOf<String?>(null)
    var export by mutableStateOf<PendingUiExport?>(null)
    var request by mutableStateOf<ImportRequest?>(null)
    var summary by mutableStateOf<ImportSummary?>(null)
    var previewAttempt by mutableIntStateOf(0)
    var feedName by mutableStateOf("")
    var feedUrl by mutableStateOf("")
    var feedTarget by mutableStateOf<String?>(null)
    var feedInterval by mutableStateOf("86400")
    var feedVisibility by mutableStateOf(SourceVisibility.household)
    var feedEnabled by mutableStateOf(true)
    var editingFeed by mutableStateOf<Source?>(null)
    var feedQueued by mutableStateOf(false)
    var windows by mutableStateOf<List<Window>?>(null)
    var availabilityVersion by mutableStateOf<Int?>(null)
    /** Local edit generation of [windows]; a save reply may replace only the generation it sent (m7c-astra C6-M2). */
    var windowsEdit by mutableIntStateOf(0)
    var windowsSavedEdit by mutableStateOf<Int?>(null)
    val windowsUnsaved get() = windowsSavedEdit != null && windowsSavedEdit != windowsEdit
    fun editWindows(value: List<Window>) { windows = value; windowsEdit++ }
    /** Refetch (offered only while no save is pending): drop local windows, their version and the unsaved marker. */
    fun resetWindows() { windows = null; availabilityVersion = null; windowsSavedEdit = null; windowsEdit++ }
    /** Runs inside the application-scoped write, so it also applies when the reply lands off-screen. */
    fun availabilitySaved(row: AvailabilityResponse, submittedEdit: Int) {
        availabilityVersion = row.version
        if (windowsEdit == submittedEdit) windows = row.windows
        windowsSavedEdit = submittedEdit
    }
}
val planningSections = listOf(SettingsSection.CalendarData, SettingsSection.Availability, SettingsSection.Planning, SettingsSection.Export, SettingsSection.Devices)

@Composable fun PlanningScreen(services: AppServices?, section: SettingsSection, fileAccess: CalendarFiles? = null) {
    val data = services?.data
    if (data == null) { Text("Sign in to manage calendar data."); return }
    val cache by data.cache.view.collectAsState()
    val ownId = cache.settings?.memberId
    val own = cache.members.find { it.id == ownId }
    val admin = own?.role == "super_admin"
    if (section == SettingsSection.Devices && !admin) {
        Text("Super admin access required."); return
    }
    var selectedMember by remember(ownId) { mutableStateOf(ownId) }
    val permittedMembers = cache.members.filter { admin || it.id == ownId }
    val member = selectedMember?.takeIf { id -> permittedMembers.any { it.id == id } } ?: ownId
    // Import/export are caller-scoped, never scoped to the non-saveable availability selector.
    val draftMember = if (section == SettingsSection.Availability || section == SettingsSection.Planning) member else ownId
    val id = "c6:${section.name}:${draftMember.orEmpty()}"
    val writer = screenWrites(id)
    val feedback by writer.state.collectAsState()
    val applicationScope = applicationWriteScope()
    val localScope = rememberCoroutineScope()
    val session = services.screenSession
    val draft = remember(session, id) { session?.planningDrafts?.getOrPut(id) { PlanningDraft() } ?: PlanningDraft() }
    var error by remember(id) { mutableStateOf<String?>(null) }
    var readBusy by remember(id) { mutableStateOf(false) }
    var sources by remember { mutableStateOf<List<Source>>(emptyList()) }
    var devices by remember { mutableStateOf<List<Device>>(emptyList()) }
    var candidates by remember { mutableStateOf<CandidatesResponse?>(null) }
    var candidateItem by remember { mutableStateOf<String?>(null) }
    var unplaced by remember { mutableStateOf<List<Unplaced>>(emptyList()) }
    var reload by remember { mutableIntStateOf(0) }
    var remove by remember { mutableStateOf<Source?>(null) }
    var revoke by remember { mutableStateOf<Device?>(null) }
    var householdExport by remember { mutableStateOf(false) }
    val files = fileAccess ?: rememberCalendarFiles()
    val busy = feedback.busy || readBusy || draft.fileBusy || feedback.retryKey != null
    suspend fun read(operation: suspend () -> Unit) {
        readBusy = true; error = null
        try { operation() } catch (e: CancellationException) { throw e }
        catch (e: ApiProblem) { error = e.detail ?: "Request rejected (${e.type}); refetch or check permissions." }
        catch (_: Exception) { error = "Unable to reach household. Cached planning remains available." }
        finally { readBusy = false }
    }
    fun submit(proposal: SuggestionResponse? = null, op: suspend (String) -> WriteResult<JsonElement>) {
        applicationScope.launch {
            error = null
            writer.submit { key ->
                try { op(key) } catch (p: ApiProblem) {
                    if (proposal != null && p.status == 422) session?.rejectSuggestion(proposal)
                    error = if (proposal != null && p.status == 422) "Proposal expired or no longer applies; suggestions refetched. Use Refetch suggestions if offline." else p.detail
                    throw p
                }
            }
        }
    }
    LaunchedEffect(id, reload, admin) { read {
        when (section) {
            SettingsSection.CalendarData -> if (admin) sources = data.feeds.list()
            SettingsSection.Availability -> member?.let { m ->
                val row = data.availability.refresh(m)
                if (draft.windows == null) { draft.windows = row.windows; draft.availabilityVersion = row.version }
            }
            SettingsSection.Planning -> data.planning.refresh(member)
            SettingsSection.Devices -> if (admin) devices = data.deviceAdmin.list()
            else -> Unit
        }
    } }
    // SAF/desktop dialogs may cover the app and trigger PIN relock. Retain the picked body in the identity-owned
    // draft, then preview only when this gated screen is composed again (never in the disposed composition scope).
    LaunchedEffect(id, draft.request, draft.previewAttempt) {
        val request = draft.request
        if (section == SettingsSection.CalendarData && request != null && draft.summary == null)
            read { draft.summary = data.imports.preview(request) }
    }
    // A destination returned while PIN-locked is retained in the session draft. Only this gated composition
    // can initiate the UI download. The platform owns the sink and aborts it on failure/cancellation.
    LaunchedEffect(session, id) { session?.cancelOrphanedPlanningExports(id) }
    LaunchedEffect(id, draft.export) {
        val pending = draft.export ?: return@LaunchedEffect
        if (!pending.begin()) return@LaunchedEffect
        try {
            if (pending.token == null) data.exports.member(pending.sink) else data.exports.household(pending.token, pending.sink)
            pending.result.complete(Unit)
        } catch (e: CancellationException) { pending.result.completeExceptionally(e); throw e }
        catch (e: Exception) { pending.result.completeExceptionally(e) }
    }
    fun saveExport(household: Boolean, token: String? = null) {
        if (draft.fileBusy) return
        draft.fileBusy = true; draft.fileError = null; draft.notice = "Choose a destination; unlock to download and validate…"
        var warning: String? = null
        try { files.save(household, { sink ->
            val pending = PendingUiExport(sink, token)
            draft.export = pending
            try { pending.awaitCompletion() } finally { if (draft.export === pending) draft.export = null }
        }, { warning = it }) { result ->
            draft.fileBusy = false
            draft.notice = if (result.getOrNull() == true) (warning ?: "Export saved") else null
            draft.fileError = result.exceptionOrNull()?.let { failure ->
                when (failure) {
                    is ApiProblem -> "Export rejected (${failure.status}): ${failure.detail ?: failure.title}"
                    is HouseholdLocked -> "Export interrupted by PIN lock. Unlock and try again."
                    is CancellationException -> "Export interrupted before completion. Try again."
                    else -> "Export failed: ${failure.message ?: "no complete download"}. No unmodified destination was removed."
                }
            }
        } } catch (e: Exception) { draft.fileBusy = false; draft.fileError = "Unable to open file dialog." }
    }
    LaunchedEffect(feedback.savedValue) {
        if (section == SettingsSection.CalendarData && draft.request != null && feedback.saved) {
            val raw = feedback.savedValue as? JsonObject
            if (raw != null && "preview" in raw) draft.summary = WireJson.decodeFromJsonElement<ImportSummary>(raw)
        }
    }
    val status: @Composable () -> Unit = {
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        draft.fileError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        draft.notice?.let { Text(it) }
        if (readBusy) Text("Loading…")
        WriteStatus(writer, onDiscard = { reload++ }, successLabel = if (section == SettingsSection.CalendarData && draft.feedQueued) "Refresh queued; polling has not completed." else "Saved")
        if (section == SettingsSection.Availability && draft.windowsUnsaved) Text("Not saved yet: edits made after the last save are kept here. Save again to send them.")
        if (feedback.conflict) {
            Text("Current authorized server value: ${feedback.current}")
            if (section == SettingsSection.Availability) {
                Text("Your windows: ${draft.windows}")
                ShellButton("Use current version for my windows", onClick = {
                    val current = feedback.current as? JsonObject
                    if (current != null) {
                        draft.availabilityVersion = current["version"]?.jsonPrimitive?.intOrNull
                        writer.discard()
                    }
                })
            }
        }
    }
    when (section) {
        SettingsSection.CalendarData -> {
            var feedsTab by remember { mutableStateOf(false) }
            Column(Modifier.fillMaxSize()) {
                Row {
                    ShellButton("ICS upload", onClick = { feedsTab = false }, selected = !feedsTab)
                    if (admin) ShellButton("Feed admin", onClick = { feedsTab = true }, selected = feedsTab)
                }
                if (!feedsTab) ImportContent(draft.summary, draft.request?.name, busy,
                    choose = choose@{ if (draft.fileBusy) return@choose
                        draft.fileBusy = true; draft.fileError = null; files.pick { result ->
                        draft.fileBusy = false
                        val picked = result.getOrNull()
                        if (result.isFailure) draft.fileError = "Choose a valid UTF-8 .ics calendar of at most 5 MiB."
                        if (picked != null && ownId != null) {
                            writer.discard()
                            draft.request = ImportRequest(picked.content, ownId, name = calendarImportName(picked.name), scope = calendarImportScope(picked.name))
                            draft.summary = null
                        }
                    } },
                    commit = { draft.request?.let { request -> draft.feedQueued = false; submit { data.imports.commit(request, it).asScreenResult() } } },
                    cancel = { draft.request = null; draft.summary = null; writer.discard() }, retry = { draft.previewAttempt++ }, feedback = status)
                else FeedContent(sources, draft.feedName, draft.feedUrl, { draft.feedName = it }, { draft.feedUrl = it },
                    add = {
                        val interval = draft.feedInterval.toIntOrNull()
                        if (interval == null || interval !in 60..2592000) error = "Refresh interval must be 60–2592000 seconds."
                        else {
                            val editing = draft.editingFeed
                            val request = SourceRequest(draft.feedName, draft.feedUrl, draft.feedTarget, draft.feedVisibility,
                                allowHttp = editing?.allowHttp ?: false, refreshSeconds = interval, enabled = draft.feedEnabled)
                            draft.feedQueued = false
                            submit { key ->
                                val result = if (editing == null) data.feeds.add(request, key) else data.feeds.update(editing, request, key)
                                if (result is WriteResult.Success) { draft.feedName = ""; draft.feedUrl = ""; draft.editingFeed = null; reload++ }
                                result.asScreenResult()
                            }
                        }
                    },
                    refresh = { row -> draft.feedQueued = true; submit { data.feeds.refresh(row, it).asScreenResult().also { reload++ } } },
                    remove = { remove = it }, reload = { reload++ }, busy = busy, feedback = status,
                    members = cache.members, target = draft.feedTarget,
                    targetChanged = { draft.feedTarget = it; draft.feedVisibility = defaultFeedVisibility(cache.members.find { member -> member.id == it }) },
                    interval = draft.feedInterval, intervalChanged = { draft.feedInterval = it },
                    visibility = draft.feedVisibility, visibilityChanged = { draft.feedVisibility = it }, enabled = draft.feedEnabled, enabledChanged = { draft.feedEnabled = it },
                    editing = draft.editingFeed != null, cancelEdit = { draft.editingFeed = null; draft.feedName = ""; draft.feedUrl = ""; writer.discard() },
                    edit = { row -> draft.editingFeed = row; draft.feedName = row.name; draft.feedUrl = row.url.orEmpty(); draft.feedTarget = row.targetMemberId
                        draft.feedInterval = row.refreshSeconds.toString(); draft.feedVisibility = row.visibility; draft.feedEnabled = row.enabled; writer.discard() }, zone = planningMemberZone(own))
            }
        }
        SettingsSection.Availability -> {
            val baseline = cache.availability.find { it.memberId == member }
            AvailabilityContent(draft.windows ?: baseline?.windows.orEmpty(), { draft.editWindows(it) },
                save = { val m = member; val version = draft.availabilityVersion ?: baseline?.version; val windows = draft.windows ?: baseline?.windows.orEmpty()
                    val submittedEdit = draft.windowsEdit
                    if (m != null && version != null) submit { key -> data.availability.put(m, version, windows.toList(), key)
                        .also { if (it is WriteResult.Success) draft.availabilitySaved(it.value, submittedEdit) }.asScreenResult() }
                    else error = "Refetch availability before saving."
                }, reload = { draft.resetWindows(); writer.discard(); reload++ },
                members = permittedMembers, member = member, select = { selectedMember = it }, busy = busy, feedback = status, zone = planningMemberZone(cache.members.find { it.id == member }))
        }
        SettingsSection.Planning -> {
            val eligible = cache.items.filter { item -> item.kind == "appointment" && item.assignees.isEmpty() && cache.members.any { it.id == item.subjectMemberId && it.role == "dependent" } }
            Column(Modifier.fillMaxSize()) {
                androidx.compose.foundation.layout.FlowRow {
                    permittedMembers.filter { it.id == ownId || it.role == "dependent" }.forEach { m ->
                        ShellButton(m.displayName, "Suggestions for ${m.displayName}", { selectedMember = m.id }, selected = member == m.id)
                    }
                }
                if (eligible.isNotEmpty()) androidx.compose.foundation.layout.FlowRow {
                    eligible.forEach { item -> ShellButton(item.title, "Candidates for ${item.title}", { candidateItem = item.id }, selected = candidateItem == item.id) }
                }
                PlanningCardsContent(cache.suggestions.filter { it.memberId == member }, { id2 -> cache.items.find { it.id == id2 }?.title ?: "Scheduled work" },
                    request = { submit { key -> data.planning.suggest(member, key).also { if (it is WriteResult.Success) unplaced = it.value.unplaced }.asScreenResult() } },
                    accept = { row -> submit(row) { data.planning.accept(row, it).asScreenResult() } }, dismiss = { row -> submit { data.planning.dismiss(row, it).asScreenResult() } },
                    candidates = candidates, candidateRequest = { val item = candidateItem ?: eligible.firstOrNull()?.id
                        if (item == null) error = "No visible unassigned dependent appointment."
                        else localScope.launch { read { candidates = data.planning.candidates(item) } }
                    }, candidateName = { id2 -> cache.members.find { it.id == id2 }?.displayName ?: "Member" }, unplaced = unplaced, busy = busy, feedback = status, reload = { reload++ },
                    memberName = { id2 -> cache.members.find { it.id == id2 }?.displayName ?: "Member" },
                    zone = { id2 -> planningMemberZone(cache.members.find { it.id == id2 }) },
                    occurrenceName = { id2 -> cache.items.find { item -> item.id == cache.occurrences.find { it.id == id2 }?.itemId }?.title ?: "Unplaced work" }, expired = { suggestionRejectionKey(it) in session?.rejectedSuggestions.orEmpty() })
            }
        }
        SettingsSection.Export -> ExportContent(admin, member = {
            saveExport(false)
        }, household = { householdExport = true }, busy = busy, feedback = status)
        SettingsSection.Devices -> DeviceAdminContent(devices, { id2 -> cache.members.find { it.id == id2 }?.displayName ?: "Member" },
            reload = { reload++ }, revoke = { revoke = it }, busy = busy, feedback = status, zone = planningMemberZone(own))
        else -> Text("Select a calendar setting.")
    }
    remove?.let { row -> AlertDialog(onDismissRequest = { remove = null }, title = { Text("Remove ${row.name}?") }, text = { Text("Polling stops and calendar events are soft-deleted. Historical data remains read-only.") },
        confirmButton = { ShellButton("Remove feed", onClick = { remove = null; draft.feedQueued = false; submit { data.feeds.remove(row, it).asScreenResult().also { reload++ } } }) },
        dismissButton = { ShellButton("Cancel", onClick = { remove = null }) }) }
    if (revoke != null || householdExport) {
        val device = revoke
        val export = householdExport
        PlanningPasswordDialog(if (export) "household export" else "revoke ${device?.label}", readBusy, problem = error,
            confirm = { password -> localScope.launch { read {
                val token = services.stepUp(password, if (export) "household.export" else "device.revoke")
                revoke = null; householdExport = false
                if (export) saveExport(true, token)
                else device?.let { row -> submit { data.deviceAdmin.revoke(row.id, token, it).asScreenResult().also { reload++ } } }
            } } }, cancel = { revoke = null; householdExport = false })
    }
}
