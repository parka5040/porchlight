package app.porchlight.shared.screens.today

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import app.porchlight.shared.theme.PorchlightTokens
import kotlinx.coroutines.launch
import kotlinx.datetime.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOf
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner

/** Upper bound between wall-clock re-checks while Today is visible (fallback for a missed wake signal). */
val TODAY_WALL_CLOCK_RECHECK = 60.seconds

@Composable fun TodayScreen(navigate: (AppRoute) -> Unit = {}, onScopeChange: () -> Unit = {}) {
    val services = LocalScreenServices.current
    val data = services?.data
    if (data == null) { EmptyState("Today", "Sign in and sync to load household data."); return }
    val view by data.cache.view.collectAsState()
    val firedOccurrences by data.firedOccurrences.collectAsState()
    val writer = screenWrites("today")
    val feedback by writer.state.collectAsState()
    val scope = applicationWriteScope()
    val settings = view.settings?.settings ?: defaultTodaySettings
    val member = view.settings?.memberId.orEmpty()
    val zone = runCatching { TimeZone.of(view.members.find { it.id == member }?.tzid ?: "UTC") }.getOrDefault(TimeZone.UTC)
    // c3a-rr1 N2: `delay` runs on monotonic time, which stops while the device sleeps, so a midnight timer alone can
    // leave yesterday's sections up for hours. Re-read the wall clock on every start/resume (Android after sleep,
    // desktop window refocus) and on every system wake signal (desktop: logind PrepareForSleep(false), which arrives
    // without any lifecycle change). While visible, the timer also re-checks the wall clock at least every
    // TODAY_WALL_CLOCK_RECHECK, which bounds a missed wake signal; it updates only when the member-local date changes.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val wakes = services.systemResumes
    val now by produceState(services.clock.now(), services.clock, zone, lifecycle, wakes) {
        combine(lifecycle.currentStateFlow, wakes ?: flowOf(0L)) { state, _ -> state }.collectLatest { state ->
            if (!state.isAtLeast(Lifecycle.State.STARTED)) return@collectLatest
            value = services.clock.now()
            while (true) {
                val wall = services.clock.now()
                val nextDay = wall.toLocalDateTime(zone).date.plus(1, DateTimeUnit.DAY).atStartOfDayIn(zone)
                delay((nextDay - wall).coerceIn(1.milliseconds, TODAY_WALL_CLOCK_RECHECK))
                val checked = services.clock.now()
                if (checked.toLocalDateTime(zone).date != value.toLocalDateTime(zone).date) value = checked
            }
        }
    }
    LaunchedEffect(settings.pinnedListId) {
        settings.pinnedListId?.let { id -> try { data.lists.get(id) } catch (e: kotlinx.coroutines.CancellationException) { throw e } catch (_: Exception) { /* keep cached persistent rows */ } }
    }
    val groups = remember(view, settings, member, zone, now) { todaySections(todayRows(view, member), settings, member, zone, now) }
    Column(Modifier.fillMaxSize().padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        DataStatus(data, retry = { scope.refreshSafely(data) })
        CollectionHeader(
            options = listOf(ScopeOption("mine", "Mine"), ScopeOption("household", "Household")),
            selectedId = settings.scope.name,
            onScopeChange = { id ->
                val selectedScope = if (id == "mine") SettingsScope.mine else SettingsScope.household
                view.settings?.takeIf { it.settings.scope != selectedScope }?.let { saved ->
                    onScopeChange()
                    scope.launch { writer.submit { data.household.putSettings(saved.version, settings.copy(scope = selectedScope), it).feedback() } }
                }
            },
            onNewItem = { navigate(AppRoute.NewItem) },
            onRefresh = { scope.refreshSafely(data) },
        )
        DesignPending(if (services.platform.kind == app.porchlight.shared.PlatformKind.Desktop) "Desktop Today adaptation" else "Mobile Today content layout")
        WriteStatus(writer)
        TodayContent(groups, settings.thisWeekDays, view.members,
            { row -> navigate(AppRoute.Item(row.id, row.occurrence?.id)) }, { row ->
                if (!feedback.busy) row.occurrence?.let { occurrence -> scope.launch {
                    writer.submit { data.occurrences.complete(occurrence.id, occurrence.version, it).feedback() }
                } }
            }, Modifier.weight(1f), take = { row -> scope.launch { writer.submit { data.items.take(row.id, row.version, it).feedback() } } }, pinned = { id ->
                val list = view.lists.find { it.id == id && (it.mode == ListMode.persistent || it.occurrenceId == null) }
                if (list == null) Text("Pinned list unavailable. Open Lists to refetch.") else {
                    ShellButton("Open ${list.name}", onClick = { navigate(AppRoute.Lists(list.id)) })
                    app.porchlight.shared.screens.lists.ListContent(list, view.members,
                        { entry, checked -> scope.launch { writer.submit { data.lists.check(entry.id, entry.version, checked, list.occurrenceId, it).feedback() } } },
                        { navigate(AppRoute.Lists(list.id)) }, { entry, index -> val replay = ListMoveReplay(); scope.launch { writer.submit(activeKey = { replay.activeKey }) { data.lists.move(list, entry, index, replay, it).feedback() } } },
                        { entry -> scope.launch { writer.submit { data.lists.archive(entry, list.occurrenceId, it).feedback() } } },
                        { scope.launch { writer.submit { data.lists.clear(list, it).feedback() } } })
                }
            }, snooze = data.localSnooze?.let { hook -> { row -> row.occurrence?.let { scope.launch { hook(it.id) } } } }, canSnooze = { row -> row.occurrence?.id in firedOccurrences })
    }
}
@Composable fun TodayContent(groups: List<TodayGroup>, days: Int, members: List<MemberRead>, open: (ItemRead) -> Unit,
    complete: (ItemRead) -> Unit, modifier: Modifier = Modifier, take: (ItemRead) -> Unit = {}, snooze: ((ItemRead) -> Unit)? = null, canSnooze: (ItemRead) -> Boolean = { true }, pinned: @Composable (String?) -> Unit = { Text("No pinned list available.") }) {
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        if (groups.isEmpty()) EmptyState("Today", "No sections enabled. Change Today settings to show sections.")
        groups.forEach { group ->
            Text(group.kind.label(days), style = MaterialTheme.typography.titleLarge)
            if (group.kind == TodaySection.pinned_list) pinned(group.pinnedListId)
            else if (group.rows.isEmpty()) Text("Nothing here.")
            group.rows.forEach { row -> PorchlightCard(Modifier.fillMaxWidth()) {
                ItemSummary(row, members)
                ShellButton("Open ${row.title}", onClick = { open(row) })
                if ("complete" in row.capabilities) ShellButton("Complete ${row.title}", onClick = { complete(row) })
                if ("take" in row.capabilities) ShellButton("Take ${row.title}", onClick = { take(row) })
                if (snooze != null && canSnooze(row) && row.occurrence?.status in setOf("todo", "doing")) ShellButton("Snooze ${row.title} 15 min", onClick = { snooze(row) })
            } }
        }
    }
}
