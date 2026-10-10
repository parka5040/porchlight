package app.porchlight.shared

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.DeviceSettings
import app.porchlight.shared.transport.ApiTransport
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class NavigationRegressionUiTest {
    @Test fun boardMineChipChangesSelectionAndProjection() = boardChips(AppRoute.Board(), "Mine", "mine", setOf("owned", "assigned", "subject"))
    @Test fun boardMemberChipChangesSelectionAndProjection() = boardChips(AppRoute.Board(), "Filter board for Blair", "other", setOf("assigned", "subject", "other"))
    @Test fun boardOwnMemberChipChangesSelectionAndProjection() = boardChips(AppRoute.Board(), "Filter board for Alex", "self", setOf("owned", "assigned", "subject"))
    @Test fun boardHouseholdChipChangesSelectionAndProjection() = boardChips(AppRoute.Board("mine"), "Household", "household", setOf("owned", "assigned", "subject", "other"))

    private fun boardChips(initial: AppRoute.Board, description: String, filter: String, expected: Set<String>) = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val fixture = FakeServices(DeviceSettings(currentRoute = initial))
        val cache = memoryCache(fixture.clock::now)
        val rows = listOf(
            itemRead("owned"),
            itemRead("assigned").copy(ownerId = "other", assignees = listOf("self")),
            itemRead("subject").copy(ownerId = "other", subjectMemberId = "self"),
            itemRead("other").copy(ownerId = "other"),
        ).map { it.copy(title = "Card ${it.id}", occurrence = occurrenceRead("occ-${it.id}", it.id)) }
        val member = MemberRead(false, "self", "home", "Alex", "user", "#FABD2F", "A", "UTC", "manual", 1)
        runBlocking { cache.install(emptySnapshot(1).copy(members = listOf(member, member.copy(id = "other", displayName = "Blair")))); cache.replaceBoard(rows) }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                check(path.startsWith("/v1/board")) { "Unexpected request: $path" }
                val selected = if ("scope=mine" in path) rows.filter { it.id != "other" } else rows
                return buildJsonObject { put("items", WireJson.encodeToJsonElement(selected)); put("next_cursor", JsonNull) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), fixture.clock::now)
        val services = object : AppServices by fixture { override val data = data }
        try {
            setContent { PorchlightApp(services) }
            waitUntil { onAllNodesWithContentDescription(description).fetchSemanticsNodes().isNotEmpty() }
            onNodeWithContentDescription(description).performClick()
            onNodeWithContentDescription(description).assertIsSelected()
            waitUntil { fixture.state.value.currentRoute == AppRoute.Board(filter) }
            for (id in rows.map { it.id }) {
                if (id in expected) onNodeWithTag("board-card-occ-$id").assertExists()
                else onNodeWithTag("board-card-occ-$id").assertDoesNotExist()
            }
            // Each chip must remain changeable, not just work once; Household must restore all cards after Mine.
            onNodeWithContentDescription("Mine").performClick()
            onNodeWithContentDescription("Mine").assertIsSelected()
            waitUntil { fixture.state.value.currentRoute == AppRoute.Board("mine") }
            onNodeWithTag("board-card-occ-other").assertDoesNotExist()
            onNodeWithContentDescription("Household").performClick()
            onNodeWithContentDescription("Household").assertIsSelected()
            waitUntil { onAllNodesWithTag("board-card-occ-other").fetchSemanticsNodes().isNotEmpty() }
            // Ordinary tab switches still restore the last explicit Board filter.
            onNodeWithContentDescription("Filter board for Blair").performClick()
            onNodeWithContentDescription("Navigate to Calendar").performClick()
            onNodeWithContentDescription("Navigate to Board").performClick()
            onNodeWithContentDescription("Filter board for Blair").assertIsSelected()
            onNodeWithTag("board-card-occ-owned").assertDoesNotExist()
        } finally { data.close() }
    }

    @Test fun settingsSystemBackPopsSectionThenListThenPreviousCalendar() = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val calendar = AppRoute.Calendar(CalendarView.Day, "2026-10-07")
        val services = FakeServices()
        lateinit var nav: androidx.navigation.NavHostController
        setContent {
            nav = androidx.navigation.compose.rememberNavController()
            app.porchlight.shared.theme.PorchlightTheme(services.state.value.themeMode, true) {
                ShellNavigation(calendar, services.state.value, services.settings, {}, nav = nav)
            }
        }
        waitUntil { onAllNodesWithText("Wed, Oct 7, 2026").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Settings").performClick()
        onNodeWithContentDescription("Notifications: open settings").performClick()
        onNodeWithText("Settings sections").assertDoesNotExist()
        // NavHost's Android/predictive back uses this same back stack, without a desktop toolbar callback.
        runOnIdle { assertTrue(nav.popBackStack()) }
        onNodeWithText("Settings sections").assertExists()
        runOnIdle { assertTrue(nav.popBackStack()) }
        onNodeWithText("Wed, Oct 7, 2026").assertExists()
    }

    @Test fun resumedSettingsSectionHasAListParent() = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val services = FakeServices(DeviceSettings(currentRoute = AppRoute.Settings(SettingsSection.Notifications)))
        setContent { PorchlightApp(services) }
        waitUntil { services.state.value.currentRoute == AppRoute.Settings(SettingsSection.Notifications) && onAllNodesWithContentDescription("Go back").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithText("Settings sections").assertDoesNotExist()
        onNodeWithContentDescription("Go back").performClick()
        onNodeWithText("Settings sections").assertExists()
        onNodeWithContentDescription("Go back").performClick()
        onNodeWithContentDescription("Navigate to Today").assertIsSelected()
    }

    @Test fun mainTabSelectionLeavesSettingsSection() = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val services = FakeServices(DeviceSettings(currentRoute = AppRoute.Calendar()))
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Settings").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Settings").performClick()
        onNodeWithContentDescription("Notifications: open settings").performClick()
        onNodeWithContentDescription("Navigate to Board").performClick()
        onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        onNodeWithContentDescription("Go back").assertDoesNotExist()
    }

    @Test fun settingsSectionBackReturnsToListThenPreviousCalendar() = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val calendar = AppRoute.Calendar(CalendarView.Day, "2026-10-07")
        val services = FakeServices(DeviceSettings(currentRoute = calendar))
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithText("Wed, Oct 7, 2026").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Settings").performClick()
        onNodeWithContentDescription("Notifications: open settings").performClick()
        waitUntil { services.state.value.currentRoute == AppRoute.Settings(SettingsSection.Notifications) }
        onNodeWithContentDescription("Notifications: open settings").assertDoesNotExist()
        onNodeWithContentDescription("Go back").performClick()
        onNodeWithText("Settings sections").assertExists()
        onNodeWithContentDescription("Notifications: open settings").assertExists()
        waitUntil { services.state.value.currentRoute == AppRoute.Settings() }
        onNodeWithContentDescription("Go back").performClick()
        onNodeWithText("Wed, Oct 7, 2026").assertExists()
        waitUntil { services.state.value.currentRoute == calendar }
    }
}
