package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiCollectionsTest {
    private val rows = List(2_000) { index -> itemRead("row-$index").copy(title = "Row $index",
        capabilities = listOf("view"), occurrence = occurrenceRead("occ-$index", "row-$index").copy(status = "todo")) }

    @Test fun todayComposesOnlyViewportRows() = runDesktopComposeUiTest(width = 320, height = 800) {
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            TodayContent(listOf(TodayGroup(TodaySection.today, rows)), 7, emptyList(), {}, {})
        } }
        val count = onAllNodes(hasText("Row", substring = true), useUnmergedTree = true).fetchSemanticsNodes().size
        assertTrue(count in 1..40, "Only viewport rows should exist, found $count nodes")
    }
    @Test fun listsComposeOnlyViewportRows() = runDesktopComposeUiTest(width = 320, height = 800) {
        val list = listFixture().copy(propertyEntries = List(2_000) { index -> entryFixture("a", "a$index").copy(id = "entry-$index", label = "Row $index") })
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            app.porchlight.shared.screens.lists.ListContent(list, emptyList(), { _, _ -> }, {}, { _, _ -> }, {}, {})
        } }
        val count = onAllNodes(hasText("Row", substring = true), useUnmergedTree = true).fetchSemanticsNodes().size
        assertTrue(count in 1..40, "Only viewport rows should exist, found $count nodes")
    }
    @Test fun boardKeepsEveryRowAvailableForFocus() = runDesktopComposeUiTest(width = 320, height = 800) {
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(boardStatuses.map { BoardColumn(it, if (it == "todo") rows else emptyList()) }, emptyList(), {}, { _, _ -> })
        } }
        assertEquals(rows.size, onAllNodes(hasContentDescription("Board card", substring = true)).fetchSemanticsNodes().size)
        onNodeWithTag("board-card-occ-1500").performScrollTo().assertIsDisplayed()
        onNodeWithTag("board-card-occ-1490").assertExists()
    }
    @Test fun persistedRouteDoesNotRecomposeAppGate() = runDesktopComposeUiTest(width = 800, height = 800) {
        val services = FakeServices()
        var compositions = 0
        setContent { CompositionLocalProvider(app.porchlight.shared.navigation.LocalShellCompositionObserver provides { compositions++ }) {
            PorchlightApp(services)
        } }
        waitForIdle()
        val before = compositions
        runOnIdle { services.state.value = services.state.value.copy(currentRoute = app.porchlight.shared.navigation.AppRoute.Inbox) }
        waitForIdle()
        assertEquals(before, compositions, "Persistence alone must not invalidate the shell")
        runOnIdle { services.state.value = services.state.value.copy(themeMode = ThemeMode.Daylight) }
        waitForIdle()
        assertTrue(compositions > before, "Actual shell settings still publish")
    }
}
