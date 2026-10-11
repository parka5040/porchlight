package app.porchlight.shared.screens

import app.porchlight.shared.theme.PorchlightTokens

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.porchlight.shared.AppServices
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.ui.*
import app.porchlight.shared.theme.LocalPorchlightPalette
import kotlinx.datetime.TimeZone
import kotlinx.coroutines.*
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.*

/** Memory-only application-scoped requests/drafts survive PIN relock and Activity recreation, never identity change. */
class ScreenSession(private val dispatcher: CoroutineDispatcher = Dispatchers.Main) {
    private var epoch: Long? = null
    var writeScope = CoroutineScope(SupervisorJob() + dispatcher)
        private set
    private val writers = mutableMapOf<String, ScreenWrites>()
    var rejectedSuggestions by mutableStateOf<Set<String>>(emptySet())
        private set
    fun rejectSuggestion(row: SuggestionResponse) { rejectedSuggestions = rejectedSuggestions + suggestionRejectionKey(row) }
    val planningDrafts = mutableMapOf<String, app.porchlight.shared.screens.planning.PlanningDraft>()
    private var cache: ReadCache? = null
    private val editors = mutableMapOf<String, RetainedCacheValue<app.porchlight.shared.screens.item.ItemEditorState>>()
    /** Compatibility/read-only view of currently authorized drafts, not a second retained copy. */
    val drafts: Map<String, app.porchlight.shared.screens.item.EditorDraft> get() {
        val view = cache?.view?.value ?: return emptyMap()
        return buildMap { editors.forEach { (id, retained) -> retained.current(view)?.takeIf { it.present }?.let {
            put(id, it.draft); put("$id:baseline", it.baseline)
        } } }
    }
    fun bindCache(value: ReadCache) { cache = value; writers.values.forEach { it.bindCache(value) } }
    fun forgetCompletedCreation(id: String, view: CacheView): Boolean {
        val editor = editors[id]?.current(view) ?: return true
        if (editor.present) return false
        editors.remove(id)
        return true
    }
    fun editor(id: String, view: CacheView, creating: Boolean): RetainedCacheValue<app.porchlight.shared.screens.item.ItemEditorState> {
        editors.entries.removeAll { it.value.current(view) == null }
        return editors.getOrPut(id) { view.retainDraft(if (creating) emptySet() else setOf(CacheResource("Item", id)),
            app.porchlight.shared.screens.item.ItemEditorState(app.porchlight.shared.screens.item.EditorDraft(
                tzid = view.members.find { it.id == view.settings?.memberId }?.tzid ?: "UTC"), creating)) }
    }
    private val uploads = mutableMapOf<String, app.porchlight.shared.screens.collaboration.UploadAttempt>()
    fun upload(id: String) = uploads.getOrPut(id) { app.porchlight.shared.screens.collaboration.UploadAttempt() }
    val attachmentPicks = app.porchlight.shared.screens.collaboration.AttachmentPicks()
    fun bind(value: Long) { if (epoch != value) { writeScope.cancel(); writeScope = CoroutineScope(SupervisorJob() + dispatcher); writers.clear(); editors.clear(); planningDrafts.clear(); uploads.clear(); attachmentPicks.clear(); rejectedSuggestions = emptySet(); epoch = value } }
    fun writes(id: String): ScreenWrites = writers.getOrPut(id) { ScreenWrites() }.also { writer -> cache?.let(writer::bindCache) }
}
fun kotlinx.coroutines.CoroutineScope.refreshSafely(data: HouseholdData) = launch {
    try { data.sync.refresh() } catch (e: CancellationException) { throw e } catch (_: Exception) { /* sync.status exposes the redacted failure */ }
}
val LocalScreenServices = staticCompositionLocalOf<AppServices?> { null }
@Composable fun screenWrites(id: String): ScreenWrites {
    val services = LocalScreenServices.current
    val fallback = remember { ScreenSession() }
    val session = services?.screenSession ?: fallback
    services?.data?.cache?.let { cache -> session.bind(cache.sessionEpoch()); session.bindCache(cache) }
    return session.writes(id)
}
inline fun <reified T> WriteResult<T>.feedback(): WriteResult<JsonElement> = when (this) {
    is WriteResult.Success -> WriteResult.Success(WireJson.encodeToJsonElement(value), refreshPending)
    is WriteResult.VersionConflict -> this
}
@Composable fun WriteStatus(writer: ScreenWrites, reapply: (() -> Unit)? = null, comparison: List<FieldComparison> = emptyList(), showFieldErrors: Boolean = true, onDiscard: () -> Unit = {}, successLabel: String = "Saved") {
    val observed by writer.state.collectAsState()
    val view = LocalScreenServices.current?.data?.cache?.view?.collectAsState()?.value
    val state = observed.let { if (view == null) it else writer.visible(view) }
    val scope = applicationWriteScope()
    Column(Modifier.fillMaxWidth().heightIn(max = PorchlightTokens.feedbackMaxHeight.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        if (state.busy) Text("Saving…")
        state.error?.let { Text(it, color = LocalPorchlightPalette.current.errorText) }
        if (showFieldErrors) state.fieldErrors.forEach { (field, error) -> Text("${fieldLabel(field)}: $error", color = LocalPorchlightPalette.current.errorText) }
        state.warning?.let { Text(it) }
        if (state.saved) Text(successLabel)
        if (state.refreshPending) Text("$successLabel; refresh pending. Do not repeat the write.")
        if (state.retryKey != null) ShellButton("Retry identical request", onClick = { scope.launchRetry(writer) })
        if (state.retryKey != null) ShellButton("Abandon retry (request may have committed)", onClick = { writer.discard(); onDiscard() })
        if (state.conflict) {
            val visibleComparison = if (state.current != null) comparison else emptyList()
            visibleComparison.forEach { diff ->
                Text(diff.label, style = MaterialTheme.typography.titleSmall)
                Text("Yours: ${diff.yours}")
                Text("Current: ${diff.current}")
            }
            if (visibleComparison.isEmpty()) Text("Current version changed. Discard and refetch before another action.")
            if (reapply != null && state.current != null) ShellButton("Reapply after comparison", onClick = reapply)
            ShellButton("Discard changes", onClick = { writer.discard(); onDiscard() })
        }
    }
}
private fun kotlinx.coroutines.CoroutineScope.launchRetry(writer: ScreenWrites) = launch { writer.retry() }
@Composable fun DataStatus(data: HouseholdData?, error: String? = null, retry: () -> Unit = {}) {
    if (data == null) { Text("Sign in and sync to load household data."); return }
    val status by data.sync.status.collectAsState()
    val view by data.cache.view.collectAsState()
    when {
        error != null -> { OfflineState(); ErrorState(error, retry) }
        status == SyncStatus.Failed("attachment-cleanup-failed") -> ErrorState("Private attachment cleanup failed. Check local storage; Porchlight will retry automatically.", retry)
        status is SyncStatus.Failed -> { OfflineState(); ErrorState("Could not refresh. Saved data may be out of date.", retry) }
        status == SyncStatus.Refreshing -> Text("Refreshing…")
        view.sync == null -> Text("Waiting for initial sync…")
    }
}
@Composable fun ItemSummary(row: ItemRead, members: List<MemberRead>) {
    Text(row.title, Modifier.fillMaxWidth(), style = MaterialTheme.typography.titleMedium)
    val view = LocalScreenServices.current?.data?.cache?.view?.collectAsState()?.value
    val memberId = view?.settings?.memberId
    val zone = runCatching { TimeZone.of(members.find { it.id == memberId }?.tzid ?: "UTC") }.getOrDefault(TimeZone.UTC)
    Text(itemTimeLabel(row, zone))
    Text(if (row.timeMode == "floating") "Floating · member local time" else "Fixed · displayed in ${zone.id}")
    if (row.pendingDerivation) Text("Pending derivation")
    row.assignees.forEach { id -> members.find { it.id == id }?.let { member ->
        MemberChip(member.displayName, members.indexOf(member), serverColor = memberColor(member.color, members.indexOf(member)), initials = member.initials)
    } }
    row.subjectMemberId?.let { id -> Text("For ${members.find { it.id == id }?.displayName ?: "Member"}") }
    row.conflicts.orEmpty().forEach { Text("Conflict warning: ${it.otherTitle}") }
}

data class FieldComparison(val label: String, val yours: String, val current: String)
fun fieldLabel(field: String): String = field.removePrefix("override_").replace('_', ' ').replaceFirstChar { it.uppercase() }
fun fieldComparisons(changes: JsonObject, current: JsonElement?): List<FieldComparison> {
    fun value(v: JsonElement?): String = when (v) {
        null -> "Unavailable"
        JsonNull -> "Cleared"
        is JsonPrimitive -> v.content
        is JsonObject -> v.entries.joinToString(", ") { (key, entry) -> "${fieldLabel(key)}: ${value(entry)}" }
        is JsonArray -> v.joinToString(", ") { value(it) }
    }
    val objectValue = current as? JsonObject
    return changes.map { (field, yours) -> FieldComparison(fieldLabel(field), value(yours), value(objectValue?.get(field))) }
}

/** Writes outlive a route; the identity epoch cancels them, not navigation or a PIN relock. */
@Composable fun applicationWriteScope(): CoroutineScope {
    val fallback = rememberCoroutineScope()
    return LocalScreenServices.current?.screenSession?.writeScope ?: fallback
}
