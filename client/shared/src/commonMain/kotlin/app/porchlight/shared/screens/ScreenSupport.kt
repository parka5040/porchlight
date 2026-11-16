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
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.serialization.json.*

/** Memory-only application-scoped requests/drafts survive PIN relock and Activity recreation, never identity change. */
class ScreenSession(private val dispatcher: CoroutineDispatcher = Dispatchers.Main) {
    private var epoch: Long? = null
    private var closed = false
    var writeScope = CoroutineScope(SupervisorJob() + dispatcher)
        private set
    private val writers = mutableMapOf<String, ScreenWrites>()
    var rejectedSuggestions by mutableStateOf<Set<String>>(emptySet())
        private set
    fun rejectSuggestion(row: SuggestionResponse) { rejectedSuggestions = rejectedSuggestions + suggestionRejectionKey(row) }
    val planningDrafts = mutableMapOf<String, app.porchlight.shared.screens.planning.PlanningDraft>()
    private var cache: ReadCache? = null
    private val editors = mutableMapOf<String, RetainedCacheValue<app.porchlight.shared.screens.item.ItemEditorState>>()
    private val discardedEditors = mutableSetOf<String>()
    fun editorDiscarded(id: String) = id in discardedEditors
    /** Compatibility/read-only view of currently authorized drafts, not a second retained copy. */
    val drafts: Map<String, app.porchlight.shared.screens.item.EditorDraft> get() {
        val view = cache?.view?.value ?: return emptyMap()
        return buildMap { editors.forEach { (id, retained) -> retained.current(view)?.takeIf { it.present }?.let {
            put(id, it.draft); put("$id:baseline", it.baseline)
        } } }
    }
    fun bindCache(value: ReadCache) { cache = value; writers.values.forEach { it.bindCache(value) } }
    fun forgetCompletedCreation(id: String, view: CacheView): Boolean {
        val retained = editors[id] ?: return true
        val editor = retained.current(view) ?: return !retained.retained(view)
        if (editor.present) return false
        editors.remove(id)
        return true
    }
    fun editor(id: String, view: CacheView, creating: Boolean): RetainedCacheValue<app.porchlight.shared.screens.item.ItemEditorState> {
        editors.entries.removeAll { (key, value) ->
            (!value.retained(view)).also { if (it) discardedEditors += key }
        }
        val retained = editors.remove(id) ?: view.retainDraft(if (creating) emptySet() else setOf(CacheResource("Item", id)),
            app.porchlight.shared.screens.item.ItemEditorState(app.porchlight.shared.screens.item.EditorDraft(
                tzid = view.members.find { it.id == view.settings?.memberId }?.tzid ?: "UTC"), creating))
        editors[id] = retained
        trim(editors, id) { _, value -> value.current(view)?.let { it.present && (it.unsaved || it.draft != it.baseline) } ?: value.retained(view) }
        return retained
    }
    private val commentEditors = mutableMapOf<String, RetainedCacheValue<app.porchlight.shared.screens.collaboration.CommentEditorState>>()
    private val selectedCommentEdits = mutableStateMapOf<String, String>()
    private val discardedCommentEditors = mutableSetOf<String>()
    fun commentEditorDiscarded(id: String) = id in discardedCommentEditors
    fun commentEditor(id: String, view: CacheView): RetainedCacheValue<app.porchlight.shared.screens.collaboration.CommentEditorState> {
        commentEditors.entries.removeAll { (key, value) ->
            (!value.retained(view)).also { if (it) discardedCommentEditors += key.substringBefore(':') }
        }
        val selected = selectedCommentEdits[id]
        val key = if (selected == null) "$id:new" else "$id:edit:$selected"
        if (selected != null && key !in commentEditors) { selectedCommentEdits.remove(id); return commentEditor(id, view) }
        val retained = commentEditors.remove(key) ?: view.retainDraft(setOf(CacheResource("Item", id)),
            app.porchlight.shared.screens.collaboration.CommentEditorState())
        commentEditors[key] = retained
        trim(commentEditors, key) { _, value -> value.current(view)?.let { draft ->
            draft.edit?.let { draft.text != it.body } ?: draft.text.isNotEmpty()
        } ?: value.retained(view) }
        selectedCommentEdits.entries.removeAll { "${it.key}:edit:${it.value}" !in commentEditors }
        return retained
    }
    fun beginCommentEdit(id: String, comment: Comment, view: CacheView) {
        val key = "$id:edit:${comment.id}"
        if (commentEditors[key]?.retained(view) != true) commentEditors[key] = view.retainDraft(
            setOf(CacheResource("Item", id), CacheResource("Comment", comment.id)),
            app.porchlight.shared.screens.collaboration.CommentEditorState().also { it.edit = comment; it.text = comment.body })
        selectedCommentEdits[id] = comment.id
    }
    fun finishCommentEdit(id: String) { selectedCommentEdits.remove(id)?.let { commentEditors.remove("$id:edit:$it") } }
    private fun clearCommentEditors() { commentEditors.clear(); selectedCommentEdits.clear(); discardedCommentEditors.clear() }
    private val uploads = mutableMapOf<String, app.porchlight.shared.screens.collaboration.UploadAttempt>()
    fun upload(id: String): app.porchlight.shared.screens.collaboration.UploadAttempt {
        val value = uploads.remove(id) ?: app.porchlight.shared.screens.collaboration.UploadAttempt()
        uploads[id] = value
        trim(uploads, id) { key, attempt -> attempt.active || writers["collaboration:$key"]?.retainsOperation == true }
        return value
    }
    private fun <T> trim(values: MutableMap<String, T>, current: String, keep: (String, T) -> Boolean) {
        var disposable = values.count { (key, value) -> !keep(key, value) }
        val iterator = values.iterator()
        while (disposable > 64 && iterator.hasNext()) {
            val entry = iterator.next()
            if (entry.key != current && !keep(entry.key, entry.value)) { iterator.remove(); disposable-- }
        }
    }
    fun close() {
        closed = true
        writeScope.cancel()
        writers.clear(); editors.clear(); discardedEditors.clear(); clearCommentEditors(); uploads.clear(); planningDrafts.clear(); attachmentPicks.clear()
        rejectedSuggestions = emptySet(); cache = null; epoch = null
    }
    val attachmentPicks = app.porchlight.shared.screens.collaboration.AttachmentPicks()
    fun bind(value: Long) { if (!closed && epoch != value) { writeScope.cancel(); writeScope = CoroutineScope(SupervisorJob() + dispatcher); writers.clear(); editors.clear(); discardedEditors.clear(); clearCommentEditors(); planningDrafts.clear(); uploads.clear(); attachmentPicks.clear(); rejectedSuggestions = emptySet(); epoch = value } }
    fun writes(id: String): ScreenWrites {
        val writer = writers.remove(id) ?: ScreenWrites()
        writers[id] = writer
        cache?.let(writer::bindCache)
        trim(writers, id) { _, writer -> writer.retainsOperation }
        return writer
    }
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
internal val LocalScreenSupportCompositionObserver = staticCompositionLocalOf<((String) -> Unit)?> { null }

@Composable fun WriteStatus(writer: ScreenWrites, reapply: (() -> Unit)? = null, comparison: List<FieldComparison> = emptyList(), showFieldErrors: Boolean = true, onDiscard: () -> Unit = {}, successLabel: String = "Saved") {
    LocalScreenSupportCompositionObserver.current?.let { observer -> SideEffect { observer("write") } }
    val cache = LocalScreenServices.current?.data?.cache
    val feedback = remember(writer, cache) {
        if (cache == null) writer.state else combine(writer.state,
            cache.view.map { it.visibilityRevision }.distinctUntilChanged()) { _, _ -> writer.visible(cache.view.value) }
            .distinctUntilChanged()
    }
    val state by feedback.collectAsState(remember(writer, cache) { writer.visible(cache?.view?.value) })
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
    LocalScreenSupportCompositionObserver.current?.let { observer -> SideEffect { observer("data") } }
    val status by data.sync.status.collectAsState()
    val hasSync by remember(data) { data.cache.view.map { it.sync != null }.distinctUntilChanged() }
        .collectAsState(remember(data) { data.cache.view.value.sync != null })
    when {
        error != null -> { OfflineState(); ErrorState(error, retry) }
        status == SyncStatus.Failed("attachment-cleanup-failed") -> ErrorState("Private attachment cleanup failed. Check local storage; Porchlight will retry automatically.", retry)
        status is SyncStatus.Failed -> { OfflineState(); ErrorState("Could not refresh. Saved data may be out of date.", retry) }
        status == SyncStatus.Refreshing -> Text("Refreshing…")
        !hasSync -> Text("Waiting for initial sync…")
    }
}
@Composable fun ItemSummary(row: ItemRead, members: List<MemberRead>) {
    LocalScreenSupportCompositionObserver.current?.let { observer -> SideEffect { observer("summary:${row.id}") } }
    Text(row.title, Modifier.fillMaxWidth(), style = MaterialTheme.typography.titleMedium)
    val cache = LocalScreenServices.current?.data?.cache
    val memberId = if (cache == null) null else {
        val selected by remember(cache) { cache.view.map { it.settings?.memberId }.distinctUntilChanged() }
            .collectAsState(remember(cache) { cache.view.value.settings?.memberId })
        selected
    }
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
    val services = LocalScreenServices.current
    val scope = services?.screenSession?.writeScope ?: fallback
    // Capture before launch: queued handlers must not adopt a replacement identity/session.
    val origin = services?.data?.cache?.writeOrigin?.invoke()
    return if (origin == null) scope else CoroutineScope(scope.coroutineContext + origin)
}
