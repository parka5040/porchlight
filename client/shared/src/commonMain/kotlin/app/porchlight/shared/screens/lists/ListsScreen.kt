package app.porchlight.shared.screens.lists

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.ImeAction
import app.porchlight.shared.theme.PorchlightTokens
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.boundsInParent
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*

// DESIGN-PENDING: desktop Lists adaptation and collaboration forms/compare/recovery.
@Composable fun ListsScreen(context: String = "Lists", occurrenceId: String? = null, navigate: (AppRoute) -> Unit = {}) {
    val services = LocalScreenServices.current
    val data = services?.data
    if (data == null) { EmptyState("Lists", "Sign in and sync to load lists."); return }
    val view by data.cache.view.collectAsState()
    var selected by remember(context) { mutableStateOf(context.takeUnless { it == "Lists" }) }
    val selectedList = view.lists.find { it.id == selected }
    val listOccurrence = if (selectedList?.mode == ListMode.persistent || selectedList?.cleanupOnly == true) null else occurrenceId
    val row = view.lists.find { it.id == selected && (it.cleanupOnly || it.mode == ListMode.persistent || it.occurrenceId == listOccurrence) }
    val writer = screenWrites("lists:${selected.orEmpty()}")
    val scope = applicationWriteScope()
    var error by remember { mutableStateOf<String?>(null) }
    var label by remember(selected) { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    val keyboard = LocalSoftwareKeyboardController.current
    var searchError by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }
    var hits by remember { mutableStateOf<RetainedCacheValue<List<ListResponse>>?>(null) }
    var editing by remember { mutableStateOf<RetainedCacheValue<EntryResponse>?>(null) }
    LaunchedEffect(view.visibilityRevision) {
        if (hits?.current(view) == null) hits = null
        if (editing?.current(view) == null) editing = null
    }
    var rename by remember(row?.id, row?.version) { mutableStateOf(row?.name.orEmpty()) }
    var deleteList by remember { mutableStateOf(false) }
    val refreshing: () -> Unit = { scope.launch { try {
        val rows = data.lists.refresh(includeOrphaned = true)
        selected?.takeUnless { id -> rows.any { it.id == id && it.cleanupOnly } }?.let { data.lists.get(it, listOccurrence) }
        error = null
    } catch (e: CancellationException) { throw e } catch (_: Exception) { error = "Could not refresh lists. Cached rows may be stale." } } }
    LaunchedEffect(data, selected, occurrenceId) {
        // Mode discovery may change listOccurrence; it must not restart this authoritative GET.
        data.cache.keepListScope(selected.orEmpty(), occurrenceId) {
            try {
                if (selected == null || row?.cleanupOnly == true) data.lists.refresh(includeOrphaned = true)
                else data.lists.get(selected!!, listOccurrence)
            } catch (e: CancellationException) { throw e } catch (_: Exception) { error = "List unavailable; refetch when connected." }
            awaitCancellation() // release the pin when navigation/identity disposes this editor
        }
    }
    val search: () -> Unit = { keyboard?.hide(); val q = query; scope.launch { try {
        hits = allSearchLists(data, q)
        searchError = null; error = null
    } catch (e: CancellationException) { throw e } catch (_: Exception) { searchError = "Search needs a connection to your household server." } } }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        if (services.platform.kind == app.porchlight.shared.PlatformKind.Desktop) DesignPending("Desktop Lists adaptation")
        SecondaryAction("Refresh lists", onClick = refreshing)
        error?.let { Text(it) }
        SegmentedChoice(view.lists.filter { it.itemId == null || it.cleanupOnly }.distinctBy { it.id }
            .sortedWith(compareBy<ListResponse> { it.displayName.lowercase() }.thenBy { it.id })
            .map { ScopeOption(it.id, it.displayName) }, selected, { selected = it })
        OutlinedTextField(query, { query = it; searchError = null }, label = { Text("Search lists") }, modifier = Modifier.fillMaxWidth(),
            singleLine = true, keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            keyboardActions = KeyboardActions(onSearch = { search() }),
            trailingIcon = { IconAction("Search lists", ActionIcon.Search, search, outlined = false) })
        searchError?.let { Text(it, color = app.porchlight.shared.theme.LocalPorchlightPalette.current.errorText) }
        hits?.current(view).orEmpty().forEach { hit -> ActionListRow("Open ${hit.name}", onClick = { scope.launch {
            try { data.lists.get(hit.id, occurrenceId); selected = hit.id; error = null }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { hits = null; error = "List unavailable; refetch when connected." }
        } }) }
        OutlinedTextField(name, { name = it }, label = { Text("New list name") }, modifier = Modifier.fillMaxWidth())
        PrimaryAction("Create list", enabled = name.isNotBlank(), onClick = { val body = CreateList(name.trim(), mode = ListMode.persistent); scope.launch { writer.submit { key ->
            val result = data.lists.create(body, key); if (result is WriteResult.Success) { selected = result.value.id; if (name.trim() == body.name) name = "" }; result.feedback()
        } } })
        WriteStatus(writer, onDiscard = refreshing)
        val state by writer.state.collectAsState()
        if (state.conflict) { Text("Current server value: ${state.current}"); Text("Refetch and compare with your draft before submitting a corrected new request.") }
        row?.let { list ->
            if (!list.cleanupOnly && list.mode == ListMode.per_occurrence && list.occurrenceId == null) Text("Template entries. Open from an item occurrence to check this instance.")
            ListContent(list, view.members, { entry, checked -> scope.launch { writer.submit { data.lists.check(entry.id, entry.version, checked, list.occurrenceId, it).feedback() } } },
                { entry -> editing = data.cache.view.value.retainCurrent {
                    lists.find { it.id == entry.listId && it.occurrenceId == entry.occurrenceId && !it.cleanupOnly && "manage" in it.capabilities }
                        ?.let { it.propertyEntries + it.gotIt }?.find { it.id == entry.id }
                } }, { entry, index -> val replay = ListMoveReplay(); scope.launch { writer.submit(activeKey = { replay.activeKey }) { data.lists.move(list, entry, index, replay, it).feedback() } } },
                { entry -> scope.launch { writer.submit { data.lists.archive(entry, list.occurrenceId, it).feedback() } } },
                { scope.launch { writer.submit { data.lists.clear(list, it).feedback() } } })
            if ("manage" in list.capabilities) {
                OutlinedTextField(label, { label = it }, label = { Text("Add to ${list.name}") }, modifier = Modifier.fillMaxWidth())
                ShellButton("Add entry", enabled = label.isNotBlank(), onClick = { val body = EntryWrite(label = label.trim()); scope.launch { writer.submit { data.lists.add(list, body, it).also { result -> if (result is WriteResult.Success && label.trim() == body.label) label = "" }.feedback() } } })
            }
            if ("edit" in list.capabilities) {
                OutlinedTextField(rename, { rename = it }, label = { Text("List name") }, modifier = Modifier.fillMaxWidth())
                ShellButton("Rename list", enabled = rename.isNotBlank(), onClick = { val body = PatchList(name = rename.trim()); scope.launch { writer.submit { data.lists.edit(list, body, it).feedback() } } })
            }
            if ("change_visibility" in list.capabilities) {
                ListVisibility.entries.forEach { visibility -> ShellButton("Visibility ${visibility.value}", selected = visibility == list.visibility, onClick = { scope.launch { writer.submit { data.lists.edit(list, PatchList(visibility = visibility), it).feedback() } } }) }
                view.members.filter { it.id != list.ownerId }.forEach { m -> ShellButton("Viewer ${m.displayName}", selected = m.id in list.viewers, onClick = {
                    val ids = if (m.id in list.viewers) list.viewers - m.id else list.viewers + m.id
                    scope.launch { writer.submit { data.lists.viewers(list, ids, it).feedback() } }
                }) }
            }
            if ("delete" in list.capabilities) ShellButton("Delete list", onClick = { deleteList = true })
            if (!list.cleanupOnly) view.settings?.let { saved -> ShellButton("Pin ${list.displayName} on Today", onClick = { scope.launch { writer.submit { data.household.putSettings(saved.version, saved.settings.copy(pinnedListId = list.id, todaySections = (saved.settings.todaySections + TodaySection.pinned_list).distinct()), it).feedback() } } }) }
            if (!list.cleanupOnly) list.itemId?.let { id -> ShellButton("Open attached item", onClick = { navigate(AppRoute.Item(id, occurrenceId)) }) }
        } ?: Text(if (selected == null) "Choose a list, or create one." else "Selected list is not cached for this occurrence.")
    }
    editing?.current(view)?.let { entry -> EntryEditor(entry, { editing = null }) { body -> scope.launch { writer.submit { data.lists.editEntry(entry, body, row?.occurrenceId, it).also { result -> if (result is WriteResult.Success) editing = null }.feedback() } } } }
    if (deleteList && row != null) AlertDialog(onDismissRequest = { deleteList = false }, title = { Text("Delete ${row.displayName}?") }, text = { Text("Entries and checks will be removed.") }, confirmButton = { ShellButton("Confirm delete list", onClick = { deleteList = false; scope.launch { writer.submit { data.lists.delete(row, it).feedback() } } }) }, dismissButton = { ShellButton("Keep list", onClick = { deleteList = false }) })
}
private suspend fun allSearchLists(data: HouseholdData, query: String): RetainedCacheValue<List<ListResponse>> = data.cache.inSession { fence ->
    val rows = allPages { cursor ->
        val page = data.search.search(query, cursor, 200)
        page.hits.mapNotNull { hit -> if (hit is SearchHit.Other && hit.entityType == "list") runCatching { app.porchlight.shared.transport.WireJson.decodeFromJsonElement<ListResponse>(hit.json) }.getOrNull() else null } to page.nextCursor
    }
    val visible = data.cache.visibleSearchLists(fence, rows)
    data.cache.retain(fence, visible, visible.flatMap { listOfNotNull(CacheResource("List", it.id), it.itemId?.let { id -> CacheResource("Item", id) }) })
}
@Composable fun ListContent(row: ListResponse, members: List<MemberRead>, check: (EntryResponse, Boolean) -> Unit,
    edit: (EntryResponse) -> Unit, move: (EntryResponse, Int) -> Unit, archive: (EntryResponse) -> Unit, clear: () -> Unit) {
    if (row.redacted == true) {
        Text("Private list", style = MaterialTheme.typography.titleLarge)
        return
    }
    val uri = LocalUriHandler.current
    val checkable = "check" in row.capabilities && (row.mode == ListMode.persistent || row.occurrenceId != null)
    val manageable = "manage" in row.capabilities
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        Text(row.displayName, style = MaterialTheme.typography.titleLarge)
        funListEntries(row.propertyEntries, manageable, checkable, members, check, edit, move, archive) { link -> runCatching { uri.openUri(link) } }
        if (row.gotIt.isNotEmpty()) {
            Text("Got it · ${row.gotIt.size}", style = MaterialTheme.typography.titleMedium)
            if (manageable) ShellButton("Clear Got it", onClick = clear)
            funListEntries(row.gotIt, manageable, checkable, members, check, edit, move, archive) { link -> runCatching { uri.openUri(link) } }
        }
        if (row.propertyEntries.isEmpty() && row.gotIt.isEmpty()) Text("No entries yet.")
    }
}
@Composable private fun funListEntries(entries: List<EntryResponse>, manageable: Boolean, checkable: Boolean,
    members: List<MemberRead>, check: (EntryResponse, Boolean) -> Unit, edit: (EntryResponse) -> Unit,
    move: (EntryResponse, Int) -> Unit, archive: (EntryResponse) -> Unit, link: (String) -> Unit) {
    val bounds = remember(entries.map { it.id }) { mutableMapOf<String, Rect>() }
    entries.forEachIndexed { index, entry ->
        val checked = entry.checkedAt != null
        val drag = if (!manageable) Modifier else Modifier.pointerInput(entry.id, entries.map { it.id }) {
            var delta = 0f
            detectDragGesturesAfterLongPress(onDragStart = { delta = 0f }, onDragCancel = { delta = 0f }, onDragEnd = {
                val center = bounds[entry.id]?.center?.y
                if (delta != 0f && center != null) {
                    val target = entries.indices.minByOrNull { i -> kotlin.math.abs((bounds[entries[i].id]?.center?.y ?: center) - (center + delta)) } ?: index
                    if (target != index) move(entry, target)
                }
            }) { change, amount -> change.consume(); delta += amount.y }
        }
        PorchlightCard(Modifier.fillMaxWidth().onGloballyPositioned { bounds[entry.id] = it.boundsInParent() }.then(drag).semantics {
            if (manageable) customActions = listOf(CustomAccessibilityAction("Move ${entry.label} up") { move(entry, (index - 1).coerceAtLeast(0)); true }, CustomAccessibilityAction("Move ${entry.label} down") { move(entry, (index + 1).coerceAtMost(entries.lastIndex)); true })
        }) {
            Text(entry.label, style = MaterialTheme.typography.titleMedium)
            if (entry.qty != null || entry.unit.isNotEmpty()) Text("${entry.qty ?: ""} ${entry.unit}")
            if (entry.notes.isNotEmpty()) Text(entry.notes)
            entry.checkedBy?.let { id -> Text("Got it · ${members.find { it.id == id }?.displayName ?: "Member"}") }
            if (checkable) ShellButton("${if (checked) "Uncheck" else "Check"} ${entry.label}", selected = checked, onClick = { check(entry, !checked) })
            entry.url?.takeIf { it.startsWith("https://") || it.startsWith("http://") }?.let { url -> ShellButton("Product link ${entry.label}", onClick = { link(url) }) }
            if (manageable) {
                ShellButton("Edit ${entry.label}", onClick = { edit(entry) })
                ShellButton("Move ${entry.label} up", enabled = index > 0, onClick = { move(entry, index - 1) })
                ShellButton("Move ${entry.label} down", enabled = index < entries.lastIndex, onClick = { move(entry, index + 1) })
                ShellButton("Archive ${entry.label}", onClick = { archive(entry) })
            }
        }
    }
}
@Composable private fun EntryEditor(entry: EntryResponse, dismiss: () -> Unit, save: (JsonObject) -> Unit) {
    var label by remember(entry.id) { mutableStateOf(entry.label) }; var qty by remember(entry.id) { mutableStateOf(entry.qty?.toString().orEmpty()) }
    var unit by remember(entry.id) { mutableStateOf(entry.unit) }; var notes by remember(entry.id) { mutableStateOf(entry.notes) }; var url by remember(entry.id) { mutableStateOf(entry.url.orEmpty()) }
    AlertDialog(onDismissRequest = dismiss, title = { Text("Edit entry") }, text = { Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        OutlinedTextField(label, { label = it }, label = { Text("Label") }); OutlinedTextField(qty, { qty = it }, label = { Text("Quantity") }); OutlinedTextField(unit, { unit = it }, label = { Text("Unit") }); OutlinedTextField(url, { url = it }, label = { Text("Product URL") }); OutlinedTextField(notes, { notes = it }, label = { Text("Notes") })
    } }, confirmButton = { ShellButton("Save entry", enabled = label.isNotBlank() && (qty.isBlank() || qty.toDoubleOrNull() != null), onClick = { save(buildJsonObject {
        put("label", label); put("qty", qty.toDoubleOrNull()?.let(::JsonPrimitive) ?: JsonNull); put("unit", unit); put("notes", notes); put("url", url.takeIf { it.isNotBlank() }?.let(::JsonPrimitive) ?: JsonNull)
    }) }) }, dismissButton = { ShellButton("Cancel entry edit", onClick = dismiss) })
}
