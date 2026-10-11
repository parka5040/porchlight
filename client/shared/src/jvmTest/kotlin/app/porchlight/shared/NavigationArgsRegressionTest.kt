package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.LocalSaveableStateRegistry
import androidx.compose.runtime.saveable.SaveableStateRegistry
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import androidx.navigation.NavHostController
import androidx.navigation.NavDestination.Companion.hasRoute
import androidx.navigation.compose.rememberNavController
import androidx.navigation.toRoute
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import kotlinx.datetime.LocalDate
import kotlinx.serialization.json.*
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import kotlin.test.*
import kotlin.time.Instant

/** Android-services-in-JVM-harness, not Android Activity/back-handler/emulator coverage. */
@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class NavigationArgsRegressionTest(private val width: Int, private val platform: PlatformKind, private val harness: String) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{0}dp-{2}")
        fun cases() = listOf(arrayOf<Any>(360, PlatformKind.Android, "Android-services-in-JVM-harness"),
            arrayOf<Any>(1440, PlatformKind.Desktop, "Desktop-JVM-harness"))
    }
    private class Fixture(platform: PlatformKind) : AutoCloseable {
        val now = Instant.parse("2026-10-09T12:00:00Z")
        val cache = memoryCache { now }
        val removed = mutableSetOf<String>()
        val listRequests = mutableListOf<String>()
        val itemReads = mutableListOf<String>()
        val rows = List(12) { i -> itemRead("card-$i").copy(title = "Fixture $i", ownerId = "self",
            occurrence = occurrenceRead("occ-$i", "card-$i").copy(dueDate = LocalDate.parse("2026-10-09"))) }
        val lists = listOf("A", "B").map { label -> listFixture().copy(id = "list-$label", name = "List $label", ownerId = "self", propertyEntries = emptyList()) }
        val attached = listFixture().copy(id = "attached", name = "Attached list", itemId = "card-0", mode = ListMode.per_occurrence, propertyEntries = emptyList())
        val members = listOf(MemberRead(false, "self", "home", "Member One", "user", "#FABD2F", "M1", "UTC", "manual", 1),
            MemberRead(false, "other", "home", "Member Two", "user", "#FABD2F", "M2", "UTC", "manual", 1))
        var settings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.pinned_list, TodaySection.today), SettingsScope.household, 7, pinnedListId = "list-A"))
        init { runBlocking {
            cache.install(emptySnapshot(1).copy(members = members, memberSettings = settings, items = rows, occurrences = rows.map { it.occurrence!! }, lists = lists + attached))
            cache.replaceBoard(rows)
        } }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                val requestPath = java.net.URLDecoder.decode(path, java.nio.charset.StandardCharsets.UTF_8)
                if (requestPath.startsWith("/v1/lists")) listRequests += requestPath
                fun page(rows: JsonElement) = buildJsonObject { put("items", rows); put("next_cursor", JsonNull) }
                return when {
                    requestPath.startsWith("/v1/board") -> page(WireJson.encodeToJsonElement(rows.filterNot { it.id in removed }))
                    requestPath.startsWith("/v1/me/settings") -> WireJson.encodeToJsonElement(settings).jsonObject
                    requestPath.startsWith("/v1/lists/") -> {
                        val id = requestPath.substringAfter("/v1/lists/").substringBefore('?')
                        val row = (lists + attached).first { it.id == id }
                        WireJson.encodeToJsonElement(row.copy(occurrenceId = requestPath.substringAfter("occurrence_id=", "").ifEmpty { null })).jsonObject
                    }
                    requestPath.startsWith("/v1/lists") -> page(WireJson.encodeToJsonElement(if ("item_id=" in requestPath) listOf(attached) else lists + attached))
                    requestPath.startsWith("/v1/items/") && "/lists" in requestPath -> page(WireJson.encodeToJsonElement(listOf(attached)))
                    requestPath.startsWith("/v1/items/") && !requestPath.contains("/comments") && !requestPath.contains("/attachments") -> {
                        val id = requestPath.substringAfter("/v1/items/").substringBefore('?').substringBefore('/')
                        if (method == "DELETE") { removed += id; throw NoContentSuccess() }
                        else {
                            itemReads += id
                            if (id in removed) throw ApiProblem("not_found", 404, "Item unavailable")
                            val row = rows.first { it.id == id }
                            WireJson.encodeToJsonElement(itemResponse(id).copy(title = row.title, ownerId = "self", capabilities = itemResponse(id).capabilities + ItemCapability.delete)).jsonObject
                        }
                    }
                    else -> page(JsonArray(emptyList()))
                }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }), { now })
        val opens = MutableStateFlow<AppRoute?>(null)
        var enforceLocalAvailability = false
        val services = object : AppServices by FakeServices() {
            override val data = this@Fixture.data
            override val pendingOpen = opens
            override val screenSession = ScreenSession()
            override val clock = AppClock { now }
            override val platform = object : PlatformInfo {
                override val kind = platform
                override val systemDark = MutableStateFlow(false)
            }
            // Non-active Board/Calendar requests below test the navigation contract, not a UI flow.
            // M-A uses production's local availability rule to cover real external item opens.
            override suspend fun routeAvailable(route: AppRoute) = !enforceLocalAvailability || localRouteAvailable(route, cache.view.value)
        }
        fun pin(id: String) = runBlocking {
            settings = settings.copy(version = settings.version + 1, settings = settings.settings.copy(pinnedListId = id))
            data.household.refreshSettings()
        }
        fun evict(id: String) = runBlocking { removed += id; cache.evictItem(id) }
        override fun close() { services.screenSession!!.writeScope.cancel(); data.close(); cache.close() }
    }
    private fun screen(start: AppRoute, test: ComposeUiTest.(Fixture, NavHostController, SaveableStateRegistry) -> Unit) {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = width, height = 1000) {
            lateinit var nav: NavHostController
            val registry = SaveableStateRegistry(null) { true }
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f), LocalScreenServices provides fixture.services,
                LocalSaveableStateRegistry provides registry) { PorchlightTheme(ThemeMode.Daylight) { Surface(Modifier.fillMaxSize()) {
                nav = rememberNavController()
                ShellNavigation(start, DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
            } } } }
            waitForIdle()
            test(fixture, nav, registry)
        } }
    }
    private fun ComposeUiTest.tab(label: String) { onNodeWithContentDescription("Navigate to $label").performClick(); waitForIdle() }
    private fun ComposeUiTest.request(fixture: Fixture, route: AppRoute) {
        runOnIdle { fixture.opens.value = route }
        waitUntil { fixture.opens.value == null }
        waitForIdle()
    }
    private fun ComposeUiTest.openCard() { onNodeWithContentDescription("Open Fixture 0").performScrollTo().performClick(); onNodeWithContentDescription("Refetch item").assertExists() }

    @Test fun i1PinnedListRequestReplacesSavedOtherListAndDetail() = screen(AppRoute.Today) { fixture, nav, _ ->
        onNodeWithContentDescription("Open List A").performScrollTo().performClick()
        onNodeWithContentDescription("List A").assertIsSelected()
        runOnIdle { nav.navigate(AppRoute.Item("card-2", "occ-2")) }
        onNodeWithContentDescription("Refetch item").assertExists()
        tab("Today")
        fixture.pin("list-B")
        onNodeWithContentDescription("Open List B").performScrollTo().performClick()
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Lists>(), "Explicit pinned list must not restore its saved item detail")
        assertEquals(AppRoute.Lists("list-B"), nav.currentBackStackEntry!!.toRoute<AppRoute.Lists>())
        onNodeWithContentDescription("List B").assertIsSelected()
        onNodeWithContentDescription("Refetch item").assertDoesNotExist()
    }
    @Test fun i1AttachedListFromOpenItemReplacesSavedOtherListAndOccurrence() = screen(AppRoute.Lists("list-A")) { fixture, nav, _ ->
        onNodeWithContentDescription("List A").assertIsSelected()
        runOnIdle { nav.navigate(AppRoute.Item("card-2", "occ-2")) }
        tab("Board")
        openCard()
        onNodeWithContentDescription("Open Attached list").performScrollTo().performClick()
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Lists>(), "Attached list navigation must not revive an old Lists detail")
        assertEquals(AppRoute.Lists("attached", "occ-0"), nav.currentBackStackEntry!!.toRoute<AppRoute.Lists>())
        assertTrue(fixture.cache.view.value.lists.any { it.id == "attached" && it.occurrenceId == "occ-0" },
            "Attached occurrence must be loaded: ${fixture.cache.view.value.lists.map { it.id to it.occurrenceId }} requests=${fixture.listRequests}")
        onNodeWithText("Attached list", substring = false).assertExists()
        onNodeWithContentDescription("Refetch item").assertDoesNotExist()
    }
    @Test fun i1ExplicitBoardArgumentsReplaceSavedTabButPlainTapRestores() = screen(AppRoute.Board("mine")) { fixture, nav, _ ->
        openCard()
        val saved = nav.currentBackStackEntry!!.id
        tab("Today"); tab("Board")
        assertEquals(saved, nav.currentBackStackEntry!!.id, "Plain tab restores its detail")
        tab("Today")
        request(fixture, AppRoute.Board("other"))
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>())
        assertEquals(AppRoute.Board("other"), nav.currentBackStackEntry!!.toRoute<AppRoute.Board>())
        onNodeWithContentDescription("Filter board for Member Two").assertIsSelected()
        // An explicit request for defaults is still not a plain tab tap.
        tab("Today"); request(fixture, AppRoute.Board())
        assertEquals(AppRoute.Board(), nav.currentBackStackEntry!!.toRoute<AppRoute.Board>())
    }
    @Test fun i1ExplicitCalendarArgumentsReplaceSavedViewDateMemberOverlayAndDetail() = screen(AppRoute.Calendar(CalendarView.Agenda, "2026-10-09", "self")) { fixture, nav, _ ->
        runOnIdle { nav.navigate(AppRoute.Item("card-2", "occ-2")) }
        onNodeWithContentDescription("Refetch item").assertExists()
        tab("Today")
        val requested = AppRoute.Calendar(CalendarView.Day, "2026-10-12", "other", "self")
        request(fixture, requested)
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Calendar>())
        assertEquals(requested, nav.currentBackStackEntry!!.toRoute<AppRoute.Calendar>())
        onNodeWithContentDescription("Day").assertIsSelected()
        onNodeWithContentDescription("Calendar for Member Two").assertIsSelected()
        tab("Today"); request(fixture, AppRoute.Calendar())
        assertEquals(AppRoute.Calendar(), nav.currentBackStackEntry!!.toRoute<AppRoute.Calendar>())
    }
    @Test fun i1ExplicitListOccurrenceAndDefaultsReplaceActiveOrSavedArguments() = screen(AppRoute.Lists("attached", "occ-0")) { fixture, nav, _ ->
        for (next in listOf(AppRoute.Lists("attached", "occ-2"), AppRoute.Lists())) {
            tab("Today"); request(fixture, next)
            assertEquals(next, nav.currentBackStackEntry!!.toRoute<AppRoute.Lists>())
        }
    }
    @Test fun i1ItemOccurrenceNewItemAndSettingsRequestsNeverRestoreUnrelatedArguments() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard(); tab("Today")
        for (item in listOf(AppRoute.Item("card-2", "occ-2"), AppRoute.Item("card-2", "alternate-occ"), AppRoute.Item("card-0"))) {
            request(fixture, item)
            assertEquals(item, nav.currentBackStackEntry!!.toRoute<AppRoute.Item>())
            onNodeWithContentDescription("Refetch item").assertExists()
        }
        request(fixture, AppRoute.NewItem)
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.NewItem>())
        onNodeWithText("Title").assertExists()
        for (section in listOf(SettingsSection.Today, SettingsSection.Appearance)) {
            request(fixture, AppRoute.Settings(section))
            assertEquals(AppRoute.Settings(section), nav.currentBackStackEntry!!.toRoute<AppRoute.Settings>())
            assertEquals(AppRoute.Settings(), nav.previousBackStackEntry!!.toRoute<AppRoute.Settings>())
        }
    }
    @Test fun i1DeletingAnItemDoesNotSaveItsDetailForBoardRestore() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard()
        // The repository evicts before its post-write refresh finishes. Hold that gap open:
        // eviction rejection must not win the race against the successful delete callback.
        val refreshFinished = kotlinx.coroutines.CompletableDeferred<Unit>()
        fixture.data.items.afterWrite = { refreshFinished.await() }
        onNodeWithContentDescription("Delete Fixture 0").performScrollTo().performClick()
        onNodeWithContentDescription("Confirm delete").performClick()
        waitUntil { "card-0" in fixture.cache.view.value.evictedItemIds }
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Item>(), "Pending local deletion must await its result, not eviction-pop")
        runOnIdle { refreshFinished.complete(Unit) }
        waitUntil { fixture.services.screenSession!!.writes("item:card-0").state.value.saved }
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "Successful delete must pop directly to its Board parent")
        onNodeWithText("This item is no longer available.").assertDoesNotExist()
        tab("Today"); tab("Board")
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "A deleted detail must never be restored")
        onNodeWithContentDescription("Refetch item").assertDoesNotExist()
        onNodeWithTag("board-card-occ-0").assertDoesNotExist()
    }
    @Test fun maOpenDetailEvictionClosesWithNotice() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard()
        fixture.evict("card-0")
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>())
        onNodeWithContentDescription("Refetch item").assertDoesNotExist()
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
        onNodeWithContentDescription("Dismiss item notice").performClick()
        onNodeWithText("This item is no longer available.").assertDoesNotExist()
        onNodeWithTag("board-columns").assertExists()
    }
    @Test fun maEvictionNoticeAlsoAppearsWhenPopRevealsAnotherDetail() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard()
        val parentDetail = nav.currentBackStackEntry!!.id
        runOnIdle { nav.navigate(AppRoute.Item("card-2")) }
        onNodeWithContentDescription("Refetch item").assertExists()
        fixture.evict("card-2")
        waitForIdle()
        assertEquals(parentDetail, nav.currentBackStackEntry!!.id)
        onNodeWithContentDescription("Refetch item").assertExists()
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
        onNodeWithContentDescription("Dismiss item notice").performClick()
        onNodeWithText("This item is no longer available.").assertDoesNotExist()
    }
    @Test fun maRefetch404ClosesDetailWithNotice() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard()
        runOnIdle { fixture.removed += "card-0" }
        onNodeWithContentDescription("Refetch item").performClick()
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>())
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
    }
    @Test fun maExplicitOpenRefetchesResharedItemDespiteStaleEviction() = screen(AppRoute.Board()) { fixture, nav, _ ->
        fixture.enforceLocalAvailability = true
        fixture.evict("card-0")
        runOnIdle { fixture.removed -= "card-0" } // Server re-shares before the stream re-admits it.
        assertTrue("card-0" in fixture.cache.view.value.evictedItemIds)
        val reads = fixture.itemReads.size
        request(fixture, AppRoute.Item("card-0", "occ-0"))
        assertTrue(fixture.itemReads.size > reads, "A fresh explicit open must refetch a stale tombstone")
        assertEquals(AppRoute.Item("card-0", "occ-0"), nav.currentBackStackEntry!!.toRoute<AppRoute.Item>())
        // The refetched item lands through a cache transaction off the Compose dispatcher; wait (bounded) for it.
        waitUntil(timeoutMillis = 5_000) { onAllNodesWithContentDescription("Edit Fixture 0").fetchSemanticsNodes().isNotEmpty() }
        assertFalse("card-0" in fixture.cache.view.value.evictedItemIds)
        // A successful explicit open is not a permanent bypass of subsequent visibility loss.
        fixture.evict("card-0")
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>())
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
    }
    @Test fun maRepeatedExplicitOpenOfCurrentItemStartsANewFetchWithoutRemounting() = screen(AppRoute.Board()) { fixture, nav, _ ->
        val requested = AppRoute.Item("card-0", "occ-0")
        request(fixture, requested)
        onNodeWithContentDescription("Refetch item").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        val identity = onNodeWithContentDescription("Refetch item").fetchSemanticsNode().id
        val reads = fixture.itemReads.size
        request(fixture, requested)
        assertTrue(fixture.itemReads.size > reads, "Each explicit request must actually run its one permitted fetch")
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        assertEquals(identity, onNodeWithContentDescription("Refetch item").fetchSemanticsNode().id)
        fixture.evict("card-0")
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "A repeat request must not leave a permanent fetching exemption")
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
    }
    @Test fun maExplicitOpenOfStillEvictedItemRefetchesThenClosesWithNotice() = screen(AppRoute.Board()) { fixture, nav, _ ->
        fixture.enforceLocalAvailability = true
        fixture.evict("card-0")
        val reads = fixture.itemReads.size
        request(fixture, AppRoute.Item("card-0"))
        assertTrue(fixture.itemReads.size > reads, "An explicit open must let the server decide visibility")
        // The server read resolves off the Compose dispatcher; wait (bounded) for the close it triggers.
        waitUntil(timeoutMillis = 5_000) { nav.currentDestination!!.hasRoute<AppRoute.Board>() }
        onNodeWithText("This item is no longer available.").assertIsDisplayed()
    }
    @Test fun mdHiddenCollectionScrollSurvivesTabSwitchAndBack() = screen(AppRoute.Board()) { _, nav, _ ->
        onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = onNodeWithTag("board-columns").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()
        assertTrue(before > 0, "Must exercise real collection scroll")
        val boardId = nav.currentBackStackEntry!!.id
        onNodeWithContentDescription("Open Fixture 8").performClick()
        onNodeWithContentDescription("Refetch item").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        if (width == 360) onNodeWithTag("board-columns").assertDoesNotExist()
        tab("Calendar"); tab("Board")
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        onNodeWithContentDescription("Refetch item").assertExists()
        if (width == 360) onNodeWithTag("board-columns").assertDoesNotExist()
        onNodeWithContentDescription("Go back").performClick()
        waitForIdle()
        assertEquals(boardId, nav.currentBackStackEntry!!.id)
        assertEquals(before, onNodeWithTag("board-columns").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value())
    }
    @Test fun mcDisposedCompositionIsNotStronglyRetainedByShellViewModel() {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = width, height = 1000) {
            lateinit var nav: NavHostController
            val mounted = mutableStateOf(true)
            val owner = object : androidx.lifecycle.ViewModelStoreOwner { override val viewModelStore = androidx.lifecycle.ViewModelStore() }
            setContent { if (mounted.value) CompositionLocalProvider(LocalDensity provides Density(1f),
                LocalScreenServices provides fixture.services, androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner provides owner) {
                PorchlightTheme(ThemeMode.Daylight) {
                    nav = rememberNavController()
                    ShellNavigation(AppRoute.Today, DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
                }
            } }
            waitForIdle()
            val state = runOnIdle { androidx.lifecycle.ViewModelProvider.create(nav.getBackStackEntry<AppRoute.Today>())[
                "shell-holder", app.porchlight.shared.navigation.ShellSavedState::class] }
            assertTrue(state.hasBoundHolder, "Exercise a bound holder")
            runOnIdle { mounted.value = false }
            waitForIdle()
            // No reflection or GC timing: the owner is intentionally still alive.
            assertFalse(state.hasBoundHolder, "A surviving ViewModel must release the disposed composition's holder")
            runOnIdle { owner.viewModelStore.clear() }
        } }
    }
    @Test fun i1EvictedItemInSavedStackIsRejectedBeforeDetailMounts() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard(); tab("Today")
        fixture.evict("card-0")
        tab("Board")
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "An evicted saved detail must be popped")
        onNodeWithContentDescription("Refetch item").assertDoesNotExist()
        assertFalse(fixture.services.screenSession!!.drafts.containsKey("card-0"))
    }
    @Test fun i1PersistedTabCannotRestoreMissingItemAfterFreshCacheAndOwnerRecreation() = coldRestore(explicit = false)
    @Test fun m1ClearingAnUnvisitedSavedTabAfterRecreationReleasesItsHolderState() = coldRestore(explicit = true)
    private fun coldRestore(explicit: Boolean) {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = width, height = 1000) {
            lateinit var nav: NavHostController
            var registry = SaveableStateRegistry(null) { true }
            var owner: androidx.lifecycle.ViewModelStoreOwner = object : androidx.lifecycle.ViewModelStoreOwner { override val viewModelStore = androidx.lifecycle.ViewModelStore() }
            val mounted = mutableStateOf(true)
            setContent { if (mounted.value) CompositionLocalProvider(LocalDensity provides Density(1f),
                LocalScreenServices provides fixture.services, LocalSaveableStateRegistry provides registry,
                androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner provides owner) {
                PorchlightTheme(ThemeMode.Daylight) { Surface(Modifier.fillMaxSize()) {
                    nav = rememberNavController()
                    ShellNavigation(AppRoute.Board(), DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
                } }
            } }
            openCard()
            val oldBoardId = nav.getBackStackEntry<AppRoute.Board>().id
            tab("Today")
            fun containsHolderKey(value: Any?): Boolean = when (value) {
                is Map<*, *> -> value.any { (key, child) -> (key == oldBoardId && child is Map<*, *>) || containsHolderKey(child) }
                is List<*> -> value.any { containsHolderKey(it) }
                else -> false
            }
            assertTrue(runOnIdle { containsHolderKey(registry.performSave()) })
            fun snapshot(value: Any?): Any? = when (value) {
                is Map<*, *> -> value.mapValues { snapshot(it.value) }
                is List<*> -> value.map { snapshot(it) }
                else -> value
            }
            val saved = runOnIdle { registry.performSave().mapValues { (_, values) -> values.map { snapshot(it) } }.also { mounted.value = false } }
            waitForIdle()
            fixture.removed += "card-0"
            runBlocking {
                fixture.cache.clear()
                fixture.cache.initialize(CacheOwner(cacheCaFingerprint("root"), null, "self")) // Re-admit the fixture owner after identity clear.
                fixture.cache.install(emptySnapshot(2).copy(members = fixture.members, memberSettings = fixture.settings,
                    items = fixture.rows.drop(1), occurrences = fixture.rows.drop(1).map { it.occurrence!! }, lists = fixture.lists))
                fixture.cache.replaceBoard(fixture.rows.drop(1))
            }
            assertTrue(fixture.cache.view.value.evictedItemIds.isEmpty(), "Simulate a cold cache without in-memory tombstones")
            runOnIdle {
                owner.viewModelStore.clear()
                owner = object : androidx.lifecycle.ViewModelStoreOwner { override val viewModelStore = androidx.lifecycle.ViewModelStore() }
                registry = SaveableStateRegistry(saved) { true }
                mounted.value = true
            }
            if (explicit) request(fixture, AppRoute.Board("mine")) else tab("Board")
            assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "Cold restored tabs must reject missing items")
            onNodeWithContentDescription("Refetch item").assertDoesNotExist()
            if (explicit) assertFalse(runOnIdle { containsHolderKey(registry.performSave()) },
                "Clearing a saved tab must release its state even when no entry owners were recreated")
            runOnIdle { mounted.value = false }
            waitForIdle()
            runOnIdle { owner.viewModelStore.clear() }
        } }
    }
    @Test fun i1EvictedCoveredDetailIsRejectedWhenBackRevealsIt() = screen(AppRoute.Board()) { fixture, nav, _ ->
        openCard()
        runOnIdle { nav.navigate(AppRoute.Item("card-2", "occ-2")) }
        fixture.evict("card-0")
        tab("Today"); tab("Board")
        assertEquals("card-2", nav.currentBackStackEntry!!.toRoute<AppRoute.Item>().id)
        runOnIdle { assertTrue(nav.popBackStack()) }
        waitForIdle()
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Board>(), "Back must skip an evicted detail below a valid detail")
    }
    @Test fun m1ClearedCollectionSaveableStateStaysBoundedOverRepeatedCycles() = screen(AppRoute.Board()) { _, nav, registry ->
        // Count holder entry keys in the public saved-state registry, not an implementation/reflection hook.
        fun entryKeys(value: Any?): Int = when (value) {
            is Map<*, *> -> value.entries.sumOf { (key, child) ->
                (if (key is String && key.matches(Regex("[0-9a-fA-F-]{36}"))) 1 else 0) + entryKeys(child)
            }
            is List<*> -> value.sumOf { entryKeys(it) }
            else -> 0
        }
        val initial = runOnIdle { entryKeys(registry.performSave()) }
        assertTrue(initial > 0, "Registry must actually contain screen state")
        repeat(12) { cycle ->
            runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
            onNodeWithContentDescription("Refetch item").assertExists()
            runOnIdle { nav.popBackStack() }
            onNodeWithTag("board-columns").assertExists()
            runOnIdle {
                nav.navigate(AppRoute.Board(if (cycle % 2 == 0) "mine" else null)) {
                    popUpTo<AppRoute.Board> { inclusive = true }
                }
            }
            onNodeWithTag("board-columns").assertExists()
            val count = runOnIdle { entryKeys(registry.performSave()) }
            assertTrue(count <= initial + 2, "Holder state must be bounded: cycle=$cycle initial=$initial current=$count")
        }
    }
    @Test fun m2PoppingCollectionUnderDetailNeverProvidesDestroyedOwners() = screen(AppRoute.Board()) { _, nav, _ ->
        openCard()
        val old = nav.getBackStackEntry<AppRoute.Board>()
        runOnIdle { assertTrue(nav.popBackStack<AppRoute.Board>(inclusive = true)) }
        waitForIdle()
        assertEquals(androidx.lifecycle.Lifecycle.State.DESTROYED, old.lifecycle.currentState)
        assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Today>())
        tab("Calendar"); tab("Board")
        onNodeWithTag("board-columns").assertExists()
    }
    @Test fun m4CollectionTabStaysSelectedWhileCompactDetailIsFullScreen() = screen(AppRoute.Board()) { _, _, _ ->
        openCard()
        onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        if (width == 360) {
            onNodeWithTag("board-columns").assertDoesNotExist()
            onNodeWithTag("detail-pane").assertDoesNotExist()
        } else onNodeWithTag("detail-pane").assertExists()
    }
}
