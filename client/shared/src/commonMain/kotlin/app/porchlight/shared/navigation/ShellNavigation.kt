package app.porchlight.shared.navigation

import app.porchlight.shared.theme.PorchlightTokens

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.*
import androidx.compose.ui.layout.*
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.platform.LocalDensity
import androidx.navigation.*
import androidx.navigation.NavDestination.Companion.hasRoute
import androidx.navigation.NavGraph.Companion.findStartDestination
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.saveable.SaveableStateHolder
import androidx.compose.runtime.saveable.mapSaver
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.savedstate.compose.LocalSavedStateRegistryOwner
import app.porchlight.shared.reportDiagnostic
import androidx.navigation.compose.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.screens.today.TodayScreen
import app.porchlight.shared.screens.board.BoardScreen
import app.porchlight.shared.screens.calendar.CalendarScreen
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.screens.inbox.InboxScreen
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.screens.settings.SettingsScreen
import app.porchlight.shared.theme.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.first
import app.porchlight.shared.data.memberZoneChanges
import kotlinx.coroutines.launch

private fun NavBackStackEntry.route(): AppRoute = when {
    destination.hasRoute<AppRoute.Today>() -> toRoute<AppRoute.Today>()
    destination.hasRoute<AppRoute.Board>() -> toRoute<AppRoute.Board>()
    destination.hasRoute<AppRoute.Calendar>() -> toRoute<AppRoute.Calendar>()
    destination.hasRoute<AppRoute.Lists>() -> toRoute<AppRoute.Lists>()
    destination.hasRoute<AppRoute.Inbox>() -> toRoute<AppRoute.Inbox>()
    destination.hasRoute<AppRoute.Settings>() -> toRoute<AppRoute.Settings>()
    destination.hasRoute<AppRoute.Item>() -> toRoute<AppRoute.Item>()
    destination.hasRoute<AppRoute.NewItem>() -> AppRoute.NewItem
    else -> AppRoute.Today
}

/** Keep an actual list parent so both the toolbar and NavHost's Android back pop section → list. */
private fun NavHostController.navigateSettings(route: AppRoute.Settings) {
    val current = currentBackStackEntry?.route()
    if (current == route) return
    if (current is AppRoute.Settings && current.section != null) {
        // Changing sections replaces only the section, never its list parent.
        popBackStack()
    }
    if (current !is AppRoute.Settings) navigate(AppRoute.Settings())
    if (route.section != null) navigate(route)
}

/** Plain tab taps restore context. A caller supplying non-default arguments is an explicit request. */
fun NavHostController.navigateTopLevel(route: AppRoute) {
    if (mainDestinations.none { it.second == route }) {
        navigateRequested(route)
        return
    }
    navigate(route) {
        popUpTo(graph.findStartDestination().id) { saveState = true }
        launchSingleTop = true
        restoreState = true
    }
}

/** Screen/deep-link actions request exact arguments, even when all arguments are defaults.
 * Only the navigation bar calls navigateTopLevel. Never let a saved stack substitute its args.
 */
private fun NavHostController.navigateRequested(route: AppRoute, discardSavedTab: (AppRoute) -> Unit = {}) {
    when (route) {
        is AppRoute.Settings -> navigateSettings(route)
        is AppRoute.Board, is AppRoute.Calendar, is AppRoute.Lists -> {
            discardSavedTab(route)
            when (route) {
                is AppRoute.Board -> clearBackStack<AppRoute.Board>()
                is AppRoute.Calendar -> clearBackStack<AppRoute.Calendar>()
                is AppRoute.Lists -> clearBackStack<AppRoute.Lists>()
                else -> Unit
            }
            val active = collectionEntry()?.takeIf { it.route()::class == route::class }
            navigate(route) {
                if (active != null) popUpTo(active.destination.id) { inclusive = true }
                else popUpTo(graph.findStartDestination().id) { saveState = true }
                launchSingleTop = true
                restoreState = false
            }
        }
        AppRoute.Today, AppRoute.Inbox -> navigateTopLevel(route)
        else -> navigate(route) { launchSingleTop = true; restoreState = false }
    }
}

/** Entry VMs retain ownership metadata, never a disposed composition's registry/Activity. */
internal class ShellSavedState : ViewModel() {
    private var holder: SaveableStateHolder? = null
    internal val hasBoundHolder: Boolean get() = holder != null
    var tabs: MutableMap<String, String> = mutableMapOf()
    fun bind(value: SaveableStateHolder) { holder = value }
    fun unbind(value: SaveableStateHolder) { if (holder === value) holder = null }
    fun remove(id: String) { holder?.removeState(id); tabs.remove(id) }
    fun discardTab(tab: String, keepId: String? = null) {
        tabs.filter { (id, owner) -> owner == tab && id != keepId }.keys.toList().forEach(::remove)
    }
}

/** Independent of NavHost's internal holder: its empty slot also installs entry owners. */
private class ShellEntryState(private val id: String, private val state: ShellSavedState) : ViewModel() {
    override fun onCleared() { state.remove(id) }
}

@Composable
private fun NavBackStackEntry.RegisterShellState(state: ShellSavedState, tab: String) {
    viewModel<ShellEntryState>(viewModelStoreOwner = this, key = "shell-state") { ShellEntryState(id, state) }
    state.tabs[id] = tab
}

@Composable
private fun NavBackStackEntry.ShellOwnersProvider(holder: SaveableStateHolder, content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalViewModelStoreOwner provides this, LocalLifecycleOwner provides this,
        LocalSavedStateRegistryOwner provides this) {
        holder.SaveableStateProvider(id, content)
    }
}

/**
 * The shell stack is Today + at most one other collection + detail/settings entries:
 * navigateTopLevel saves/pops the old tab, and filter changes replace the active collection.
 * Public typed lookups therefore find the nearest collection by checking non-start tabs
 * before Today. They return the actual entry (including its arguments/id), never a saved,
 * inactive tab. This also works when a restored stack has several details above the tab;
 * no in-memory last-tab pointer or duplicate saved state needs to survive recreation.
 */
private fun NavHostController.collectionEntry(): NavBackStackEntry? =
    mainDestinations.asReversed().firstNotNullOfOrNull { (_, destination) ->
        try { getBackStackEntry(destination::class) }
        catch (_: IllegalArgumentException) { null } // This collection is not on the active stack.
    }

/** An explicit open gets one fetch past a tombstone; restored/Back-revealed entries do not. */
private data class FreshItemRequest(val entryId: String, val route: AppRoute.Item,
    val fetching: Boolean = true, val loadKey: Any = Any())

@Composable
private fun rejectedItem(services: app.porchlight.shared.AppServices?, entry: NavBackStackEntry?, route: AppRoute,
    fresh: FreshItemRequest?): ItemUnavailableReason? {
    val views = services?.data?.cache?.view
    if (route !is AppRoute.Item || views == null) return null
    return key(entry?.id, route, fresh) {
        val request = fresh?.takeIf { it.entryId == entry?.id && it.route == route }
        // Read the collected cache only inside derivedStateOf. The shell observes just the
        // rejection reason, including the initial value, not every unrelated cache publication.
        val cacheState = views.collectAsState()
        val rejected by remember(cacheState) { derivedStateOf { itemUnavailable(cacheState.value, route, request) } }
        rejected
    }
}

private enum class ItemUnavailableReason(val message: String) {
    Evicted("This item is no longer available."),
    Uncached("This item is not in saved data. Reconnect and open it again.")
}
private fun itemUnavailable(cache: app.porchlight.shared.data.CacheView, route: AppRoute.Item, request: FreshItemRequest?) = when {
    route.id in cache.evictedItemIds && request?.fetching != true -> ItemUnavailableReason.Evicted
    !localRouteAvailable(route, cache) && request == null -> ItemUnavailableReason.Uncached
    else -> null
}

@Composable
fun ShellNavigation(startRoute: AppRoute, settings: DeviceSettings, store: SettingsStore, savedRoute: (AppRoute) -> Unit,
    nav: NavHostController = rememberNavController(), safeInsets: WindowInsets = WindowInsets.safeDrawing,
    loadErrorRetry: (() -> Unit)? = null, accountContent: (@Composable () -> Unit)? = null,
    services: app.porchlight.shared.AppServices? = null) { // Navigation's saveable back stack survives Android process death.
    var launchApplied by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(nav) {
        if (!launchApplied) {
            nav.currentBackStackEntryFlow.first() // NavHost installs the graph after the initial composition.
            // The graph always starts at Today, so back from any top-level tab reaches Today.
            // Do not replace a Navigation back stack restored by rememberNavController.
            if (nav.previousBackStackEntry == null && startRoute != AppRoute.Today) {
                if (mainDestinations.any { it.second.title() == startRoute.title() }) nav.navigateTopLevel(startRoute)
                else if (startRoute is AppRoute.Settings) nav.navigateSettings(startRoute)
                else nav.navigate(startRoute) { launchSingleTop = true }
            }
            launchApplied = true
        }
    }
    val observedEntry by nav.currentBackStackEntryAsState()
    // Safety comes from synchronous public back-queue lookups: popped entries leave the queue
    // before being destroyed. Never retain owners from an asynchronously observed lifecycle.
    val entry = observedEntry?.let { nav.currentBackStackEntry }
    val route = entry?.route() ?: startRoute
    var freshItemRequest by remember { mutableStateOf<FreshItemRequest?>(null) }
    var deletingItem by remember { mutableStateOf<Pair<String, AppRoute.Item>?>(null) }
    var unavailableNoticeAt by remember { mutableStateOf<String?>(null) }
    var unavailableMessage by remember { mutableStateOf("") }
    val rejectedItem = rejectedItem(services, entry, route, freshItemRequest).takeUnless { deletingItem == (entry?.id to route) }
    LaunchedEffect(entry, route) {
        if (freshItemRequest?.let { it.entryId == entry?.id && it.route == route } != true) freshItemRequest = null
        if (unavailableNoticeAt != entry?.id) unavailableNoticeAt = null
    }
    LaunchedEffect(entry, rejectedItem) {
        // Keep the intended eviction-pop, but replace silent disappearance with a status notice.
        // Local deletion waits for its own result/callback, rather than racing this rejection.
        if (rejectedItem != null && nav.currentBackStackEntry == entry) {
            unavailableMessage = rejectedItem.message
            nav.popBackStack()
            unavailableNoticeAt = nav.currentBackStackEntry?.id
        }
    }
    // The public current-entry state invalidates these lookups on push, Back and tab restore.
    val collectionEntry = if (entry == null) null else nav.collectionEntry()
    val collectionRoute = collectionEntry?.route() ?: AppRoute.Today
    val isDetail = route is AppRoute.Item || route == AppRoute.NewItem
    val selectedTitle = if (isDetail) collectionRoute.title() else route.title()
    val screenStates = rememberSaveableStateHolder()
    // A cold saved tab has no entry VMs until visited. Persist only ownership metadata so an
    // explicit clear can also release those unvisited keys; Navigation still owns all routes.
    val stateTabs = rememberSaveable(saver = mapSaver(
        save = { map: MutableMap<String, String> -> map.toMap() },
        restore = { map -> map.mapValues { it.value as String }.toMutableMap() },
    )) { mutableMapOf<String, String>() }
    val shellState = entry?.let { current ->
        val rootOwner = remember(nav, current) { nav.getBackStackEntry<AppRoute.Today>() }
        viewModel<ShellSavedState>(viewModelStoreOwner = rootOwner, key = "shell-holder") { ShellSavedState() }
            .also { state -> state.tabs = stateTabs }
    }
    DisposableEffect(shellState, screenStates) {
        shellState?.bind(screenStates)
        onDispose { shellState?.unbind(screenStates) }
    }
    val latestShellState by rememberUpdatedState(shellState)
    shellState?.let { state ->
        // Only entries from current back-queue lookups reach owners; key their registrations too.
        // Register the collection even when compact detail/settings hide its composition.
        collectionEntry?.let { parent -> key(parent.id) { parent.RegisterShellState(state, collectionRoute.title()) } }
        if (entry != collectionEntry) entry?.let { current -> key(current.id) { current.RegisterShellState(state, collectionRoute.title()) } }
    }
    val shellLifecycleOwner = LocalLifecycleOwner.current
    val scope = rememberCoroutineScope()
    var failedWrite by remember { mutableStateOf<(suspend () -> Unit)?>(null) }
    suspend fun write(block: suspend () -> Unit) {
        try { block(); failedWrite = null }
        catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (e: Exception) { reportDiagnostic("Saving device settings", e); failedWrite = block }
    }
    LaunchedEffect(nav, launchApplied) {
        if (!launchApplied) return@LaunchedEffect
        nav.currentBackStackEntryFlow.collect { current ->
            val r = current.route()
            savedRoute(r)
            write { store.saveRoute(r) } // serialized collector: each navigation change is persisted, in order.
        }
    }
    fun navigate(r: AppRoute) {
        scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) {
            nav.navigateRequested(r) { latestShellState?.discardTab(it.title()) }
            unavailableNoticeAt = null
            freshItemRequest = if (r is AppRoute.Item) nav.currentBackStackEntry?.id?.let { FreshItemRequest(it, r) } else null
        }
    }
    LaunchedEffect(services) {
        services?.pendingOpen?.collect { requested -> if (requested != null) {
            // An explicit item open is not resume: stale local eviction cannot veto its fetch.
            if (requested is AppRoute.Item || services.routeAvailable(requested)) navigate(requested)
            services.pendingOpen?.value = null
        } }
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val expanded = maxWidth >= PorchlightTokens.expandedBreakpoint.dp
        val palette = LocalPorchlightPalette.current
        val hasDetail = expanded && isDetail
        val showUnavailableNotice = unavailableNoticeAt != null && unavailableNoticeAt == entry?.id
        val unavailableNotice: @Composable () -> Unit = {
            Column(Modifier.fillMaxWidth().padding(PorchlightTokens.spaceMd.dp)
                .semantics { liveRegion = LiveRegionMode.Polite }, verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
                EmptyState("Item unavailable", unavailableMessage)
                ShellButton("Dismiss", "Dismiss item notice", { unavailableNoticeAt = null })
            }
        }
        val collection: @Composable (AppRoute) -> Unit = { destination ->
            when (destination) {
                AppRoute.Today -> TodayScreen(::navigate, onScopeChange = {
                    // Same policy as Board/Calendar: an explicit scope change closes selection.
                    scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) {
                        val current = nav.currentBackStackEntry?.route()
                        if (current is AppRoute.Item || current == AppRoute.NewItem) {
                            // Cold covered details have no entry VM yet. Retire their holder keys
                            // explicitly while preserving the permanent Today root's scroll state.
                            val today = nav.getBackStackEntry<AppRoute.Today>()
                            latestShellState?.discardTab(AppRoute.Today.title(), keepId = today.id)
                            nav.popBackStack<AppRoute.Today>(inclusive = false)
                        }
                    }
                })
                is AppRoute.Board -> BoardScreen(destination.filter, ::navigate)
                is AppRoute.Calendar -> CalendarScreen(services, destination, { navigate(it) },
                    { navigate(it) }, { navigate(AppRoute.Settings(it)) })
                is AppRoute.Lists -> ListsScreen(destination.listId ?: "Lists", destination.occurrenceId, ::navigate)
                AppRoute.Inbox -> InboxScreen(::navigate)
                else -> TodayScreen(::navigate)
            }
        }
        Row(Modifier.fillMaxSize().windowInsetsPadding(safeInsets).testTag("shell-safe-area")) {
            if (expanded) {
                Column(Modifier.width(PorchlightTokens.railWidth.dp).fillMaxHeight().background(palette.navigationSurface).padding(PorchlightTokens.spaceInset.dp),
                    verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
                    Text("Porchlight", style = MaterialTheme.typography.titleLarge, color = palette.heading,
                        modifier = Modifier.padding(PorchlightTokens.spaceSm.dp))
                    mainDestinations.forEach { (label, r) ->
                        NavEntry(label, selectedTitle == label, false, { scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) { nav.navigateTopLevel(r) } }, Modifier.fillMaxWidth())
                    }
                    Spacer(Modifier.weight(1f))
                    ShellButton("Settings", "Settings", { navigate(AppRoute.Settings()) })
                }
            }
            Column(Modifier.weight(1f).fillMaxHeight()) {
                Row(Modifier.fillMaxWidth().padding(horizontal = PorchlightTokens.spaceMd.dp, vertical = PorchlightTokens.spaceSm.dp)
                    // Opening detail adds Back; reserve its token height only in the two-pane shell
                    // so focused collection content isn't nudged by a shrinking scroll viewport.
                    .then(if (expanded) Modifier.heightIn(min = PorchlightTokens.touchTarget.dp) else Modifier),
                    horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (route is AppRoute.Item || route is AppRoute.Settings || route == AppRoute.NewItem) {
                        ShellButton("Back", "Go back", {
                            scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) {
                                if (!nav.popBackStack()) nav.navigate(AppRoute.Today) { launchSingleTop = true }
                            }
                        })
                    }
                    ShellTitle(route.title(), !expanded, { navigate(AppRoute.Settings()) }, Modifier.weight(1f))
                }
                services?.updates?.let { app.porchlight.shared.update.UpdateBanner(it, settings.dismissedUpdate) }
                services?.runtimeNotice?.let { flow ->
                    val notice by flow.collectAsState()
                    notice?.let { Text(it, modifier = Modifier.fillMaxWidth().padding(PorchlightTokens.spaceMd.dp)) }
                }
                services?.data?.live?.let { live ->
                    val since by live.unreachableSince.collectAsState()
                    val zoneFlow = remember(services.data) { services.data!!.cache.view.memberZoneChanges() }
                    val zone by zoneFlow.collectAsState(kotlinx.datetime.TimeZone.currentSystemDefault().id)
                    app.porchlight.shared.screens.inbox.ReachabilityBanner(since, zone) {
                        live.networkChanged()
                        scope.launch { try { services.data?.sync?.refresh() }
                            catch (e: kotlinx.coroutines.CancellationException) { throw e } catch (_: Exception) {} }
                    }
                }
                if (loadErrorRetry != null) {
                    PorchlightCard(Modifier.fillMaxWidth().padding(horizontal = PorchlightTokens.spaceMd.dp)) {
                        Text("Device settings could not be loaded.", color = palette.errorText)
                        ShellButton("Try again", "Try again: retry loading", loadErrorRetry)
                    }
                }
                failedWrite?.let { pending ->
                    PorchlightCard(Modifier.fillMaxWidth().padding(horizontal = PorchlightTokens.spaceMd.dp)) {
                        Text("Device settings could not be saved.", color = palette.errorText)
                        ShellButton("Try again", "Try again: retry saving device settings", { scope.launch { write(pending) } })
                    }
                }
                Box(Modifier.weight(1f).fillMaxWidth().testTag("shell-content")) {
                    // Navigation still owns routes, entry lifetimes, tab restoration and system Back.
                    // Screen rendering is deliberately outside its single-current-entry content slot:
                    // otherwise opening a detail disposes the visible collection even on a wide screen.
                    NavHost(navController = nav, startDestination = AppRoute.Today, modifier = Modifier.matchParentSize(),
                        enterTransition = { androidx.compose.animation.EnterTransition.None },
                        exitTransition = { androidx.compose.animation.ExitTransition.None },
                        popEnterTransition = { androidx.compose.animation.EnterTransition.None },
                        popExitTransition = { androidx.compose.animation.ExitTransition.None }) {
                        composable<AppRoute.Today> {}
                        composable<AppRoute.Board> {}
                        composable<AppRoute.Calendar>(typeMap = routeTypes) {}
                        composable<AppRoute.Lists> {}
                        composable<AppRoute.Inbox> {}
                        composable<AppRoute.Item> {}
                        composable<AppRoute.NewItem> {}
                        composable<AppRoute.Settings>(typeMap = routeTypes) {}
                    }
                    Row(Modifier.fillMaxSize()) {
                        if (route !is AppRoute.Settings && (expanded || !isDetail)) {
                            Column(Modifier.weight(1f).fillMaxHeight()) {
                                if (!expanded && showUnavailableNotice) unavailableNotice()
                                collectionEntry?.let { parent -> key(parent.id) {
                                    parent.ShellOwnersProvider(screenStates) {
                                        // This entry is still visible beside a detail, although Navigation's
                                        // single-pane lifecycle regards it as covered. Keep Today wake/date
                                        // observation tied to the actual visible shell, not CREATED.
                                        CompositionLocalProvider(LocalLifecycleOwner provides shellLifecycleOwner) {
                                            collection(parent.route())
                                        }
                                    }
                                } }
                            }
                        }
                        if (expanded && showUnavailableNotice && !isDetail) {
                            Box(Modifier.widthIn(max = PorchlightTokens.detailWidth.dp).weight(0.65f).fillMaxHeight()
                                .background(palette.surface)) { unavailableNotice() }
                        }
                        if (isDetail && rejectedItem == null) {
                            // One hosting location on both sides of the breakpoint. Only sizing and
                            // decoration change; even remember-only editor state keeps its identity.
                            val detailModifier = if (hasDetail) Modifier.widthIn(max = PorchlightTokens.detailWidth.dp)
                                .weight(0.65f).background(palette.surface).padding(PorchlightTokens.spaceMd.dp).testTag("detail-pane")
                                else Modifier.weight(1f)
                            Column(detailModifier.fillMaxHeight(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceMd.dp)) {
                                if (showUnavailableNotice) unavailableNotice()
                                if (hasDetail) {
                                    Text("Details", style = MaterialTheme.typography.headlineSmall)
                                    DesignPending("Detail pane")
                                }
                                entry?.let { detail -> key(detail.id) {
                                    detail.ShellOwnersProvider(screenStates) {
                                        // launchSingleTop may update arguments on the same entry. A
                                        // different item/occurrence must not inherit its editor flags.
                                        val selected = detail.route()
                                        key(selected) {
                                            when (selected) {
                                                is AppRoute.Item -> ItemScreen(selected.id, selected.occurrenceId, navigate = ::navigate,
                                                    loadKey = freshItemRequest?.takeIf { it.entryId == detail.id && it.route == selected }?.loadKey,
                                                    onDeleted = {
                                                        // Match entry as well as arguments: a late write cannot pop a newer visit.
                                                        if (nav.currentBackStackEntry?.let { it.id == detail.id && it.route() == selected } == true) nav.popBackStack()
                                                    },
                                                    onDeletePendingChange = { pending ->
                                                        val request = detail.id to selected
                                                        if (pending) deletingItem = request
                                                        else if (deletingItem == request) deletingItem = null
                                                    },
                                                    onLoadFinished = {
                                                        freshItemRequest?.takeIf { it.entryId == detail.id && it.route == selected }
                                                            ?.let { freshItemRequest = it.copy(fetching = false) }
                                                    })
                                                AppRoute.NewItem -> ItemScreen("new", creating = true, navigate = ::navigate)
                                                else -> Unit
                                            }
                                        }
                                    }
                                } }
                            }
                        }
                        if (route is AppRoute.Settings) {
                            entry?.let { current -> key(current.id) {
                                current.ShellOwnersProvider(screenStates) {
                                    SettingsScreen(settings, route.section,
                                        { mode -> scope.launch { write { store.setTheme(mode) } } },
                                        { screen -> scope.launch { write { store.setStartScreen(screen) } } },
                                        { section -> navigate(AppRoute.Settings(section)) }, accountContent, services)
                                }
                            } }
                        }
                    }
                }
                if (!expanded) {
                    val measurer = rememberTextMeasurer()
                    val density = LocalDensity.current
                    val style = MaterialTheme.typography.labelSmall
                    val minimums = remember(measurer, density, style) {
                        with(density) { mainDestinations.map { (label, _) ->
                            maxOf(PorchlightTokens.touchTarget.dp.roundToPx(),
                                measurer.measure(label, style, softWrap = false).size.width + PorchlightTokens.spaceXs.dp.roundToPx() * 2)
                        } }
                    }
                    BoxWithConstraints(Modifier.fillMaxWidth().background(palette.navigationSurface)) {
                        val viewportWidth = constraints.maxWidth
                        val labelsFit = viewportWidth >= minimums.sum()
                        val scroll = rememberScrollState()
                        val floor = with(density) { PorchlightTokens.touchTarget.dp.roundToPx() }
                        val widths = if (labelsFit) minimums else List(mainDestinations.size) { floor }
                        val contentWidth = maxOf(viewportWidth, widths.sum())
                        val spare = contentWidth - widths.sum()
                        val selectedIndex = mainDestinations.indexOfFirst { it.first == selectedTitle }
                        LaunchedEffect(selectedIndex, viewportWidth, widths, scroll.maxValue) {
                            if (selectedIndex >= 0) {
                                val left = widths.take(selectedIndex).sum() + spare * selectedIndex / widths.size
                                val right = widths.take(selectedIndex + 1).sum() + spare * (selectedIndex + 1) / widths.size
                                val offset = when {
                                    right - left > viewportWidth || left < scroll.value -> left
                                    right > scroll.value + viewportWidth -> right - viewportWidth
                                    else -> scroll.value
                                }
                                scroll.scrollTo(offset)
                            }
                        }
                        Layout(modifier = Modifier.horizontalScroll(scroll), content = {
                            mainDestinations.forEach { (label, r) ->
                                NavEntry(label, selectedTitle == label, true,
                                    { scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) { nav.navigateTopLevel(r) } },
                                    Modifier, showLabel = labelsFit)
                            }
                        }) { items, constraints ->
                            val width = contentWidth
                            var used = 0
                            val places = items.mapIndexed { index, item ->
                                val end = widths.take(index + 1).sum() + spare * (index + 1) / items.size
                                item.measure(Constraints.fixedWidth(end - used)).also { used = end }
                            }
                            val height = constraints.constrainHeight(places.maxOf { it.height })
                            layout(width, height) {
                                var x = 0
                                places.forEach { it.placeRelative(x, (height - it.height) / 2); x += it.width }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** Retain the text action unless the smallest title token needs that space. */
@Composable internal fun ShellTitle(title: String, showSettings: Boolean, openSettings: () -> Unit, modifier: Modifier = Modifier) {
    val styles = listOf(MaterialTheme.typography.headlineMedium, MaterialTheme.typography.titleLarge, MaterialTheme.typography.titleMedium)
    val palette = LocalPorchlightPalette.current
    Layout(modifier = modifier, content = {
        TokenFittingText(title, styles, palette.heading)
        if (showSettings) {
            // A single, persistent accessible action. Both visual alternatives are always cleared;
            // the measure policy never needs to change semantics or write composition state.
            Layout(modifier = Modifier.semantics(mergeDescendants = true) {
                contentDescription = "Settings"
                role = Role.Button
                onClick { openSettings(); true }
            }, content = {
                ShellButton("Settings", "Settings", openSettings, Modifier.clearAndSetSemantics {})
                IconAction("Settings", ActionIcon.Settings, openSettings, Modifier.clearAndSetSemantics {}, outlined = false)
            }, measurePolicy = remember { SettingsActionPolicy() })
        }
    }, measurePolicy = remember { ShellTitlePolicy() })
}

private class SettingsActionPolicy : MeasurePolicy {
    private fun choice(items: List<IntrinsicMeasurable>, width: Int) =
        if (items[0].maxIntrinsicWidth(Constraints.Infinity) <= width) items[0] else items[1]
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val action = (choice(measurables, constraints.maxWidth) as Measurable).measure(constraints)
        return layout(action.width, action.height) { action.placeRelative(0, 0) }
    }
    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) = measurables[1].minIntrinsicWidth(height)
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) = measurables[0].maxIntrinsicWidth(height)
    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = choice(measurables, width).minIntrinsicHeight(width)
    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = choice(measurables, width).maxIntrinsicHeight(width)
}

private class ShellTitlePolicy : MeasurePolicy {
    private fun Density.gap(items: List<IntrinsicMeasurable>) = if (items.size > 1) PorchlightTokens.spaceSm.dp.roundToPx() else 0
    private fun Density.actionWidth(items: List<IntrinsicMeasurable>, width: Int): Int {
        val action = items.getOrNull(1) ?: return 0
        val natural = action.maxIntrinsicWidth(Constraints.Infinity)
        return if (items[0].minIntrinsicWidth(Constraints.Infinity) + natural + gap(items) <= width) natural
            else action.minIntrinsicWidth(Constraints.Infinity)
    }
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val width = if (constraints.hasBoundedWidth) constraints.maxWidth else
            measurables[0].maxIntrinsicWidth(Constraints.Infinity) + gap(measurables) + (measurables.getOrNull(1)?.maxIntrinsicWidth(Constraints.Infinity) ?: 0)
        val actionWidth = actionWidth(measurables, width).coerceAtMost(width)
        val action = measurables.getOrNull(1)?.measure(constraints.copy(minWidth = actionWidth, maxWidth = actionWidth, minHeight = 0))
        val titleWidth = (width - (action?.width ?: 0) - gap(measurables)).coerceAtLeast(0)
        val title = measurables[0].measure(constraints.copy(minWidth = titleWidth, maxWidth = titleWidth, minHeight = 0))
        val height = constraints.constrainHeight(maxOf(title.height, action?.height ?: 0))
        return layout(width, height) {
            title.placeRelative(0, (height - title.height) / 2)
            action?.placeRelative(width - action.width, (height - action.height) / 2)
        }
    }
    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables[0].minIntrinsicWidth(height) + gap(measurables) + (measurables.getOrNull(1)?.minIntrinsicWidth(height) ?: 0)
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables[0].maxIntrinsicWidth(height) + gap(measurables) + (measurables.getOrNull(1)?.maxIntrinsicWidth(height) ?: 0)
    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int {
        val actionWidth = actionWidth(measurables, width)
        return maxOf(measurables[0].minIntrinsicHeight((width - gap(measurables) - actionWidth).coerceAtLeast(0)), measurables.getOrNull(1)?.minIntrinsicHeight(actionWidth) ?: 0)
    }
    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = minIntrinsicHeight(measurables, width)
}

@Composable
private fun NavEntry(label: String, isSelected: Boolean, compact: Boolean, onClick: () -> Unit, modifier: Modifier, showLabel: Boolean = true) {
    val palette = LocalPorchlightPalette.current
    val color = if (isSelected) palette.primaryText else palette.onSurfaceMuted
    val container = modifier.heightIn(min = PorchlightTokens.touchTarget.dp).widthIn(min = PorchlightTokens.touchTarget.dp)
        .background(if (isSelected) palette.selectedSurface else palette.navigationSurface, MaterialTheme.shapes.small)
        .clickable(onClick = onClick).semantics { contentDescription = "Navigate to $label"; selected = isSelected; role = Role.Tab }
        .padding(horizontal = PorchlightTokens.spaceXs.dp, vertical = PorchlightTokens.spaceSm.dp)
    if (compact) Column(container, horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceXs.dp, Alignment.CenterVertically)) {
        NavigationIcon(label, color)
        if (showLabel) TokenFittingText(label, listOf(MaterialTheme.typography.labelMedium, MaterialTheme.typography.labelSmall), color,
            Modifier.fillMaxWidth(), Modifier.testTag("nav-label-$label"), androidx.compose.ui.text.style.TextAlign.Center)
    } else Row(container.padding(horizontal = PorchlightTokens.spaceSm.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        NavigationIcon(label, color)
        TokenFittingText(label, listOf(MaterialTheme.typography.labelMedium, MaterialTheme.typography.labelSmall), color,
            Modifier.fillMaxWidth(), Modifier.testTag("nav-label-$label"))
    }
}

/** Small bundled line icons, rather than adding a Material-icons dependency. Labels carry semantics. */
@Composable private fun NavigationIcon(label: String, color: androidx.compose.ui.graphics.Color) {
    Canvas(Modifier.size(PorchlightTokens.iconSize.dp)) {
        val s = size.width / 24f
        val stroke = Stroke(2f * s)
        when (label) {
            "Today" -> { drawCircle(color, 5f*s, center, style = stroke)
                for (i in 0..7) {
                    val a = i * kotlin.math.PI / 4
                    drawLine(color, center + Offset(kotlin.math.cos(a).toFloat()*8*s, kotlin.math.sin(a).toFloat()*8*s),
                        center + Offset(kotlin.math.cos(a).toFloat()*11*s, kotlin.math.sin(a).toFloat()*11*s), 2*s)
                }
            }
            "Board" -> for (i in 0..2) drawRect(color, Offset((3+i*7)*s,4*s), Size(4*s, (14-i*2)*s), style=stroke)
            "Calendar" -> { drawRect(color,Offset(3*s,4*s), Size(18*s,17*s),style=stroke); drawLine(color,Offset(3*s,9*s),Offset(21*s,9*s),2*s) }
            "Lists" -> for (i in 0..2) { drawCircle(color,1*s,Offset(4*s,(5+i*7)*s)); drawLine(color,Offset(8*s,(5+i*7)*s),Offset(21*s,(5+i*7)*s),2*s) }
            else -> { drawRect(color,Offset(3*s,5*s),Size(18*s,15*s),style=stroke); drawLine(color,Offset(7*s,13*s),Offset(17*s,13*s),2*s) }
        }
    }
}
