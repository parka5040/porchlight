package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.LocalSaveableStateRegistry
import androidx.compose.runtime.saveable.SaveableStateRegistry
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.*
import androidx.navigation.NavHostController
import androidx.navigation.compose.rememberNavController
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
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

/** Real screens, real Navigation back stack, local in-memory transport only. */
@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class ShellStateRegressionTest(private val width: Int, private val platform: PlatformKind, private val harness: String) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{0}dp-{2}")
        fun cases() = listOf(arrayOf<Any>(1440, PlatformKind.Desktop, "Desktop-JVM-harness"),
            arrayOf<Any>(1024, PlatformKind.Android, "Android-services-in-JVM-harness"))
    }
    private class Fixture(platform: PlatformKind) : AutoCloseable {
        val now = Instant.parse("2026-10-09T12:00:00Z")
        val cache = memoryCache { now }
        val boardReads = AtomicInteger()
        val agendaReads = AtomicInteger()
        val members = listOf(MemberRead(false, "self", "home", "Member One", "user", "#FABD2F", "M1", "UTC", "manual", 1),
            MemberRead(false, "other", "home", "Member Two", "user", "#FABD2F", "M2", "UTC", "manual", 1))
        val rows = List(18) { index -> itemRead("card-$index").copy(title = "Fixture $index", ownerId = if (index % 2 == 0) "self" else "other",
            occurrence = occurrenceRead("occ-$index", "card-$index", index.toString().padStart(2, '0')).copy(dueDate = LocalDate.parse("2026-10-09"))) }
        var settings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.today), SettingsScope.household, 7))
        init { runBlocking { cache.install(emptySnapshot(1).copy(members = members, memberSettings = settings, items = rows, occurrences = rows.map { it.occurrence!! })); cache.replaceBoard(rows) } }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                fun page(rows: JsonElement) = buildJsonObject { put("items", rows); put("next_cursor", JsonNull) }
                return when {
                    path.startsWith("/v1/board") -> { boardReads.incrementAndGet(); page(WireJson.encodeToJsonElement(if ("scope=mine" in path) rows.filter { it.ownerId == "self" } else rows)) }
                    path.startsWith("/v1/agenda") -> { agendaReads.incrementAndGet(); page(WireJson.encodeToJsonElement(rows.map { it.copy(assignees = listOf("self"), allDay = true, agenda = agenda(it.occurrence!!.id, it.id, due = LocalDate.parse("2026-10-09")).agenda) })) }
                    path.startsWith("/v1/me/settings") -> {
                        if (method == "PUT") settings = settings.copy(version = settings.version + 1, settings = WireJson.decodeFromJsonElement(body!!))
                        WireJson.encodeToJsonElement(settings).jsonObject
                    }
                    path.startsWith("/v1/items/") && !path.contains("/lists") && !path.contains("/comments") && !path.contains("/attachments") -> {
                        val id = path.substringAfter("/v1/items/").substringBefore('?').substringBefore('/')
                        val row = rows.first { it.id == id }
                        WireJson.encodeToJsonElement(itemResponse(id).copy(title = row.title, ownerId = row.ownerId)).jsonObject
                    }
                    else -> page(JsonArray(emptyList()))
                }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = this@Fixture.data
            override val screenSession = ScreenSession()
            override val clock = AppClock { now }
            override val platform = object : PlatformInfo {
                override val kind = platform
                override val systemDark = MutableStateFlow(false)
            }
        }
        override fun close() { services.screenSession!!.writeScope.cancel(); data.close(); cache.close() }
    }
    private fun screen(route: AppRoute, test: ComposeUiTest.(Fixture, NavHostController, MutableState<Dp>) -> Unit) {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = maxOf(width, 1440), height = 1000) {
            lateinit var nav: NavHostController
            val available = mutableStateOf(width.dp)
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f), LocalScreenServices provides fixture.services) {
                PorchlightTheme(ThemeMode.Daylight) { Surface(Modifier.width(available.value).fillMaxHeight()) {
                    nav = rememberNavController()
                    ShellNavigation(route, DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
                } }
            } }
            waitForIdle()
            test(fixture, nav, available)
        } }
    }
    private fun ComposeUiTest.collectionScroll(board: Boolean) = if (board) onNodeWithTag("board-columns") else onNode(
        SemanticsMatcher("scrollable collection content") { (it.config.getOrNull(SemanticsProperties.VerticalScrollAxisRange)?.maxValue?.invoke() ?: 0f) > 0f } and !hasAnyAncestor(hasTestTag("detail-pane")))
    private fun SemanticsNodeInteraction.offset() = fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()

    @Test fun ni1BoardScrollAndCompositionSurviveOpenSwitchAndCloseWithoutRefresh() = stableScroll(true)
    @Test fun ni1TodayScrollAndCompositionSurviveOpenSwitchAndClose() = stableScroll(false)
    private fun stableScroll(board: Boolean) = screen(if (board) AppRoute.Board() else AppRoute.Today) { fixture, nav, _ ->
        waitUntil { onAllNodesWithContentDescription("Open Fixture 8").fetchSemanticsNodes().isNotEmpty() }
        val scroll = collectionScroll(board)
        onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = scroll.offset()
        val identity = scroll.fetchSemanticsNode().id
        val reads = fixture.boardReads.get()
        assertTrue(before > 0)
        onNodeWithContentDescription("Open Fixture 8").performClick()
        onNodeWithTag("detail-pane").assertExists()
        fun unchanged() {
            assertEquals(identity, collectionScroll(board).fetchSemanticsNode().id, "Same mounted collection, not a restored duplicate")
            assertEquals(before, collectionScroll(board).offset(), "Collection scroll survives detail navigation")
            assertEquals(reads, fixture.boardReads.get(), "Detail must not refresh Board")
        }
        unchanged()
        onNodeWithContentDescription("Open Fixture 12").performSemanticsAction(SemanticsActions.OnClick) { it() }
        unchanged()
        // Close the detail stack exactly as system Back does, checking each intermediate selection too.
        runOnIdle { nav.popBackStack() }
        unchanged()
        if (onAllNodesWithTag("detail-pane").fetchSemanticsNodes().isNotEmpty()) runOnIdle { nav.popBackStack() }
        onNodeWithTag("detail-pane").assertDoesNotExist()
        unchanged()
    }
    @Test fun boardDetailOnDetailThenBackTwiceKeepsMountedCollectionAndScroll() = stackedDetails(true)
    @Test fun todayDetailOnDetailThenBackTwiceKeepsMountedCollectionAndScroll() = stackedDetails(false)
    private fun stackedDetails(board: Boolean) = screen(if (board) AppRoute.Board() else AppRoute.Today) { fixture, nav, _ ->
        onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = collectionScroll(board).offset()
        val identity = collectionScroll(board).fetchSemanticsNode().id
        val collectionId = nav.currentBackStackEntry!!.id
        val reads = fixture.boardReads.get()
        assertTrue(before > 0)
        fun unchanged() {
            assertEquals(identity, collectionScroll(board).fetchSemanticsNode().id)
            assertEquals(before, collectionScroll(board).offset())
            assertEquals(reads, fixture.boardReads.get())
        }
        onNodeWithContentDescription("Open Fixture 8").performClick()
        onNodeWithTag("detail-pane").assertExists()
        val firstDetailId = nav.currentBackStackEntry!!.id
        unchanged()
        // Do not use the collection's single-top action: exercise two actual detail entries.
        runOnIdle { nav.navigate(AppRoute.Item("card-12")) }
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(firstDetailId, nav.previousBackStackEntry!!.id)
        assertNotEquals(firstDetailId, nav.currentBackStackEntry!!.id)
        unchanged()
        runOnIdle { assertTrue(nav.popBackStack()) }
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(firstDetailId, nav.currentBackStackEntry!!.id)
        unchanged()
        runOnIdle { assertTrue(nav.popBackStack()) }
        onNodeWithTag("detail-pane").assertDoesNotExist()
        assertEquals(collectionId, nav.currentBackStackEntry!!.id)
        unchanged()
    }
    @Test fun tabRestoreFindsCollectionBelowTwoDetailsAndIgnoresInactiveTabs() = screen(AppRoute.Board("mine")) { _, nav, _ ->
        onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = collectionScroll(true).offset()
        val boardId = nav.currentBackStackEntry!!.id
        assertTrue(before > 0)
        runOnIdle { nav.navigate(AppRoute.Item("card-8")); nav.navigate(AppRoute.Item("card-12")) }
        onNodeWithTag("detail-pane").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        onNodeWithContentDescription("Navigate to Calendar").performClick()
        runOnIdle { nav.navigate(AppRoute.Item("card-0")); nav.navigate(AppRoute.Item("card-2")) }
        onNodeWithContentDescription("Navigate to Calendar").assertIsSelected()
        onNodeWithTag("calendar-scroll").assertExists()
        onNodeWithTag("board-columns").assertDoesNotExist()
        onNodeWithContentDescription("Navigate to Board").performClick()
        onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        onNodeWithContentDescription("Mine").assertIsSelected()
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        assertEquals(boardId, nav.getBackStackEntry<AppRoute.Board>().id)
        assertEquals(before, collectionScroll(true).offset())
        val restoredIdentity = collectionScroll(true).fetchSemanticsNode().id
        repeat(2) {
            runOnIdle { assertTrue(nav.popBackStack()) }
            assertEquals(restoredIdentity, collectionScroll(true).fetchSemanticsNode().id)
            assertEquals(before, collectionScroll(true).offset())
        }
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Navigate to Today").performClick()
        runOnIdle { nav.navigate(AppRoute.Item("card-0")); nav.navigate(AppRoute.Item("card-2")) }
        onNodeWithContentDescription("Navigate to Today").assertIsSelected()
        onNodeWithTag("detail-pane").assertExists()
        onNodeWithTag("board-columns").assertDoesNotExist()
        onNodeWithTag("calendar-scroll").assertDoesNotExist()
    }
    @Test fun savedStateRecreationFindsCollectionBelowTwoDetailsAndRestoresScroll() {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = width, height = 1000) {
            lateinit var nav: NavHostController
            var registry = SaveableStateRegistry(null) { true }
            var owner: ViewModelStoreOwner = object : ViewModelStoreOwner { override val viewModelStore = ViewModelStore() }
            val mounted = mutableStateOf(true)
            setContent {
                if (mounted.value) CompositionLocalProvider(LocalDensity provides Density(1f),
                    LocalScreenServices provides fixture.services, LocalSaveableStateRegistry provides registry,
                    LocalViewModelStoreOwner provides owner) {
                    PorchlightTheme(ThemeMode.Daylight) { Surface(Modifier.fillMaxSize()) {
                        nav = rememberNavController()
                        ShellNavigation(AppRoute.Board("mine"), DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
                    } }
                }
            }
            onNodeWithContentDescription("Open Fixture 8").performScrollTo()
            val before = collectionScroll(true).offset()
            val collectionId = nav.currentBackStackEntry!!.id
            assertTrue(before > 0)
            runOnIdle { nav.navigate(AppRoute.Item("card-8")); nav.navigate(AppRoute.Item("card-12")) }
            onNodeWithTag("detail-pane").assertExists()
            val previousNav = nav
            val detailId = nav.currentBackStackEntry!!.id
            // Recreate both the saveable composition and ViewModel owners, not just the layout.
            // performSave exposes mutable holder maps in this Compose version. A real process
            // snapshot serializes by value; don't let clearing the old store mutate our snapshot.
            fun snapshot(value: Any?): Any? = when (value) {
                is Map<*, *> -> value.mapValues { snapshot(it.value) }
                is List<*> -> value.map { snapshot(it) }
                else -> value
            }
            val saved = runOnIdle { registry.performSave().mapValues { (_, values) -> values.map { snapshot(it) } }.also { mounted.value = false } }
            waitForIdle()
            runOnIdle {
                owner.viewModelStore.clear()
                owner = object : ViewModelStoreOwner { override val viewModelStore = ViewModelStore() }
                registry = SaveableStateRegistry(saved) { true }
                mounted.value = true
            }
            onNodeWithTag("detail-pane").assertExists()
            assertNotSame(previousNav, nav)
            assertEquals(detailId, nav.currentBackStackEntry!!.id)
            assertEquals(collectionId, nav.getBackStackEntry<AppRoute.Board>().id)
            onNodeWithContentDescription("Navigate to Board").assertIsSelected()
            onNodeWithContentDescription("Mine").assertIsSelected()
            assertEquals(before, collectionScroll(true).offset())
            val restoredIdentity = collectionScroll(true).fetchSemanticsNode().id
            repeat(2) {
                runOnIdle { assertTrue(nav.popBackStack()) }
                assertEquals(restoredIdentity, collectionScroll(true).fetchSemanticsNode().id)
                assertEquals(before, collectionScroll(true).offset())
            }
            onNodeWithTag("detail-pane").assertDoesNotExist()
            assertEquals(collectionId, nav.currentBackStackEntry!!.id)
            runOnIdle { mounted.value = false }
            waitForIdle()
            runOnIdle { owner.viewModelStore.clear() }
        } }
    }
    @Test fun n3TodayScopeChangeAfterColdRecreationReleasesCoveredDetailState() {
        Fixture(platform).use { fixture -> runDesktopComposeUiTest(width = width, height = 1000) {
            lateinit var nav: NavHostController
            var registry = SaveableStateRegistry(null) { true }
            var owner: ViewModelStoreOwner = object : ViewModelStoreOwner { override val viewModelStore = ViewModelStore() }
            val mounted = mutableStateOf(true)
            setContent { if (mounted.value) CompositionLocalProvider(LocalDensity provides Density(1f),
                LocalScreenServices provides fixture.services, LocalSaveableStateRegistry provides registry,
                LocalViewModelStoreOwner provides owner) { PorchlightTheme(ThemeMode.Daylight) {
                nav = rememberNavController()
                ShellNavigation(AppRoute.Today, DeviceSettings(), fixture.services.settings, {}, nav = nav, services = fixture.services)
            } } }
            onNodeWithContentDescription("Open Fixture 0").performClick()
            onNodeWithTag("detail-pane").assertExists()
            val coveredId = nav.currentBackStackEntry!!.id
            runOnIdle { nav.navigate(AppRoute.Item("card-2")) }
            onNodeWithTag("detail-pane").assertExists()
            fun hasCoveredState(value: Any?): Boolean = when (value) {
                is Map<*, *> -> value.any { (key, child) -> (key == coveredId && child is Map<*, *>) || hasCoveredState(child) }
                is List<*> -> value.any { hasCoveredState(it) }
                else -> false
            }
            fun snapshot(value: Any?): Any? = when (value) {
                is Map<*, *> -> value.mapValues { snapshot(it.value) }
                is List<*> -> value.map { snapshot(it) }
                else -> value
            }
            val saved = runOnIdle { registry.performSave().mapValues { (_, values) -> values.map { snapshot(it) } }.also { mounted.value = false } }
            waitForIdle()
            runOnIdle {
                owner.viewModelStore.clear()
                owner = object : ViewModelStoreOwner { override val viewModelStore = ViewModelStore() }
                registry = SaveableStateRegistry(saved) { true }
                mounted.value = true
            }
            onNodeWithTag("detail-pane").assertExists()
            assertTrue(runOnIdle { hasCoveredState(registry.performSave()) }, "Cold covered detail really has saved holder state")
            onNodeWithContentDescription("Mine").performClick()
            onNodeWithTag("detail-pane").assertDoesNotExist()
            assertFalse(runOnIdle { hasCoveredState(registry.performSave()) }, "Today scope pop must release unregistered covered detail state")
            runOnIdle { mounted.value = false }
            waitForIdle()
            runOnIdle { owner.viewModelStore.clear() }
        } }
    }
    @Test fun ni1RememberOnlyListDraftSurvivesOpenSwitchClose() = screen(AppRoute.Lists()) { _, nav, _ ->
        onNodeWithText("New list name").performTextInput("Unsubmitted fixture draft")
        val field = onNodeWithText("New list name").fetchSemanticsNode().id
        for (id in listOf("card-0", "card-2")) {
            runOnIdle { nav.navigate(AppRoute.Item(id)) }
            onNodeWithText("Unsubmitted fixture draft").assertExists()
            assertEquals(field, onNodeWithText("New list name").fetchSemanticsNode().id)
        }
        repeat(2) { runOnIdle { nav.popBackStack() }; onNodeWithText("Unsubmitted fixture draft").assertExists() }
    }
    @Test fun ni1CalendarViewScrollAndIdentitySurviveDetail() = screen(AppRoute.Calendar(CalendarView.Agenda, "2026-10-09")) { _, nav, _ ->
        val scroll = onNodeWithTag("calendar-scroll")
        onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = scroll.offset()
        assertTrue(before > 0, "Calendar scroll retention must exercise a nonzero offset")
        val identity = scroll.fetchSemanticsNode().id
        runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(identity, scroll.fetchSemanticsNode().id)
        assertEquals(before, scroll.offset())
        onNodeWithContentDescription("Agenda").assertIsSelected()
        runOnIdle { nav.popBackStack() }
        assertEquals(identity, scroll.fetchSemanticsNode().id)
        assertEquals(before, scroll.offset())
    }
    @Test fun ni2BoardFilterChangesCloseDetailAndApplyImmediately() = screen(AppRoute.Board()) { _, _, _ ->
        for ((description, expected) in listOf("Mine" to "self", "Filter board for Member Two" to "other", "Household" to "all")) {
            onNodeWithContentDescription(if (expected == "other") "Open Fixture 0" else "Open Fixture 1").performClick()
            onNodeWithTag("detail-pane").assertExists()
            onNodeWithContentDescription(description).performClick()
            onNodeWithContentDescription(description).assertIsSelected()
            onNodeWithTag("detail-pane").assertDoesNotExist()
            waitUntil { (expected != "all" || onAllNodesWithTag("board-card-occ-0").fetchSemanticsNodes().isNotEmpty()) &&
                (expected != "other" || onAllNodesWithTag("board-card-occ-1").fetchSemanticsNodes().isNotEmpty()) }
            if (expected == "self") onNodeWithTag("board-card-occ-1").assertDoesNotExist()
            if (expected == "other") onNodeWithTag("board-card-occ-0").assertDoesNotExist()
        }
        onNodeWithContentDescription("Navigate to Calendar").performClick()
        onNodeWithContentDescription("Navigate to Board").performClick()
        onNodeWithContentDescription("Household").assertIsSelected()
        onNodeWithTag("detail-pane").assertDoesNotExist()
    }
    @Test fun ni2TodayScopeClosesDetailAndUpdatesProjection() = screen(AppRoute.Today) { fixture, _, _ ->
        onNodeWithContentDescription("Open Fixture 1").performClick()
        onNodeWithTag("detail-pane").assertExists()
        onNodeWithContentDescription("Mine").performClick()
        waitUntil { fixture.cache.view.value.settings?.settings?.scope == SettingsScope.mine }
        onNodeWithContentDescription("Mine").assertIsSelected()
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Open Fixture 1").assertDoesNotExist()
        onNodeWithContentDescription("Open Fixture 0").assertExists()
    }
    @Test fun ni2CalendarChangesCloseDetailAndApplyImmediately() = screen(AppRoute.Calendar(CalendarView.Agenda, "2026-10-09")) { _, nav, _ ->
        runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
        onNodeWithTag("detail-pane").assertExists()
        onNodeWithContentDescription("Day").performClick()
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Day").assertIsSelected()
        runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
        onNodeWithTag("detail-pane").assertExists()
        onNodeWithContentDescription("Calendar for Member Two").performClick()
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Calendar for Member Two").assertIsSelected()
    }
    @Test fun m3ReselectingBoardScopeIsNoOpIncludingDefaultHousehold() = screen(AppRoute.Board()) { fixture, nav, _ ->
        for (description in listOf("Household", "Mine", "Filter board for Member Two")) {
            if (description != "Household") onNodeWithContentDescription(description).performClick()
            onNodeWithContentDescription(description).assertIsSelected()
            val collectionId = nav.getBackStackEntry<AppRoute.Board>().id
            val identity = onNodeWithTag("board-columns").fetchSemanticsNode().id
            val reads = fixture.boardReads.get()
            runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
            onNodeWithTag("detail-pane").assertExists()
            val detailId = nav.currentBackStackEntry!!.id
            onNodeWithContentDescription(description).performClick()
            onNodeWithTag("detail-pane").assertExists()
            assertEquals(detailId, nav.currentBackStackEntry!!.id)
            assertEquals(collectionId, nav.getBackStackEntry<AppRoute.Board>().id)
            assertEquals(identity, onNodeWithTag("board-columns").fetchSemanticsNode().id)
            assertEquals(reads, fixture.boardReads.get())
            runOnIdle { nav.popBackStack() }
        }
    }
    @Test fun m3ReselectingTodayScopeIsNoOpAndSubmitsNoWrite() = screen(AppRoute.Today) { fixture, nav, _ ->
        onNodeWithContentDescription("Open Fixture 8").performScrollTo().performClick()
        onNodeWithTag("detail-pane").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        val identity = collectionScroll(false).fetchSemanticsNode().id
        val before = collectionScroll(false).offset()
        assertTrue(before > 0, "Today re-selection must preserve a nonzero offset")
        val version = fixture.settings.version
        onNodeWithContentDescription("Household").performClick()
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        assertEquals(identity, collectionScroll(false).fetchSemanticsNode().id)
        assertEquals(before, collectionScroll(false).offset())
        assertEquals(version, fixture.settings.version)
    }
    @Test fun m3ReselectingCalendarViewIsNoOpEvenWithImplicitDateAndMember() = screen(AppRoute.Calendar()) { _, nav, _ ->
        val identity = onNodeWithTag("calendar-scroll").fetchSemanticsNode().id
        val collectionId = nav.getBackStackEntry<AppRoute.Calendar>().id
        runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
        onNodeWithTag("detail-pane").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        onNodeWithContentDescription("Week").performClick()
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        assertEquals(collectionId, nav.getBackStackEntry<AppRoute.Calendar>().id)
        assertEquals(identity, onNodeWithTag("calendar-scroll").fetchSemanticsNode().id)
    }
    @Test fun mbReselectingCalendarMemberWithImplicitArgumentsIsNoOp() = calendarReselect(AppRoute.Calendar(), "Calendar for Member One")
    @Test fun mbReselectingCalendarMemberPreservesAgendaOffset() = calendarReselect(AppRoute.Calendar(CalendarView.Agenda), "Calendar for Member One", scroll = true)
    @Test fun mbTodayWithImplicitDateAndMemberIsNoOp() = calendarReselect(AppRoute.Calendar(), "Today")
    @Test fun mbTodayInSameWeekWithDifferentAnchorIsNoOp() = calendarReselect(AppRoute.Calendar(date = "2026-10-07"), "Today")
    @Test fun mbTodayPreservesAgendaOffset() = calendarReselect(AppRoute.Calendar(CalendarView.Agenda), "Today", scroll = true)
    private fun calendarReselect(route: AppRoute.Calendar, description: String, scroll: Boolean = false) = screen(route) { fixture, nav, _ ->
        val collection = onNodeWithTag("calendar-scroll")
        if (scroll) onNodeWithContentDescription("Open Fixture 8").performScrollTo()
        val before = collection.offset()
        if (scroll) assertTrue(before > 0, "Calendar re-selection must exercise nonzero scroll")
        val identity = collection.fetchSemanticsNode().id
        val collectionId = nav.getBackStackEntry<AppRoute.Calendar>().id
        val reads = fixture.agendaReads.get()
        assertTrue(reads > 0, "Calendar has finished its first refresh")
        runOnIdle { nav.navigate(AppRoute.Item("card-0")) }
        onNodeWithTag("detail-pane").assertExists()
        val detailId = nav.currentBackStackEntry!!.id
        // Semantics click avoids scrolling the control into view changing the tested offset.
        onNodeWithContentDescription(description).performSemanticsAction(SemanticsActions.OnClick) { it() }
        onNodeWithTag("detail-pane").assertExists()
        assertEquals(detailId, nav.currentBackStackEntry!!.id)
        assertEquals(collectionId, nav.getBackStackEntry<AppRoute.Calendar>().id)
        assertEquals(identity, collection.fetchSemanticsNode().id)
        assertEquals(before, collection.offset())
        assertEquals(reads, fixture.agendaReads.get(), "Re-selection must not refresh Calendar")
    }
    @Test fun ni3NewItemDraftAndEditorIdentitySurviveBothBreakpointDirections() = screen(AppRoute.Board()) { _, _, available ->
        onNodeWithContentDescription("New item").performClick()
        onNodeWithText("Title").performTextInput("New fixture draft")
        val identity = onNodeWithText("Title").fetchSemanticsNode().id
        for (next in listOf(360, width, 600, width)) {
            runOnIdle { available.value = next.dp }
            onNodeWithText("New fixture draft").assertExists()
            assertEquals(identity, onNodeWithText("Title").fetchSemanticsNode().id, "Editor must not be rehosted at the breakpoint")
        }
    }
    @Test fun ni3ExistingItemEditingStateSurvivesBothBreakpointDirections() = screen(AppRoute.Board()) { _, _, available ->
        onNodeWithContentDescription("Open Fixture 0").performClick()
        onNodeWithContentDescription("Edit Fixture 0").performScrollTo().performClick()
        onNodeWithText("Title").performTextReplacement("Edited fixture draft")
        for (next in listOf(360, width)) {
            runOnIdle { available.value = next.dp }
            onNodeWithText("Edited fixture draft").assertExists()
            onNodeWithContentDescription("Cancel editor").assertExists()
        }
    }
    @Test fun compactPhoneStillUsesFullScreenDetailAndBackRestoresCollection() = screen(AppRoute.Board()) { _, _, available ->
        runOnIdle { available.value = 360.dp }
        onNodeWithContentDescription("Open Fixture 0").performClick()
        onNodeWithTag("board-columns").assertDoesNotExist()
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Refetch item").assertExists()
        onNodeWithContentDescription("Go back").performClick()
        onNodeWithTag("board-columns").assertExists()
    }
}

@OptIn(ExperimentalTestApi::class)
class CompactSelectionRegressionTest {
    @Test fun ni5SelectedTabIsVisibleOnFirstLayoutAndEverySelectionAt160dp() {
        for (scale in listOf(1f, 2f)) runDesktopComposeUiTest(width = 160, height = 800) {
            lateinit var nav: NavHostController
            val fixture = FakeServices()
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale)) { PorchlightTheme(ThemeMode.Daylight) {
                nav = rememberNavController()
                ShellNavigation(AppRoute.Inbox, DeviceSettings(), fixture.settings, {}, nav = nav)
            } } }
            fun visible(label: String) {
                val selected = onNodeWithContentDescription("Navigate to $label").assertIsSelected().assertIsDisplayed()
                val clipped = selected.fetchSemanticsNode().boundsInRoot
                assertEquals(selected.getUnclippedBoundsInRoot().width.value, clipped.width, "Whole selected tab is visible")
                assertTrue(clipped.left >= 0 && clipped.right <= 160)
            }
            visible("Inbox")
            for ((label, route) in mainDestinations + mainDestinations.reversed()) {
                runOnIdle { nav.navigateTopLevel(route) }
                visible(label)
            }
        }
    }
}
