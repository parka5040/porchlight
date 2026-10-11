package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.data.itemRead
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.screens.settingsRead
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.api.models.*
import kotlinx.datetime.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class C3bUiTest {
    @Test fun calendarHasAccessibleControlsAndServerLabelsAtDoubleScaleBothThemes() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 1000, height = 1600) {
            val row = itemRead("one", true).copy(title = "Meds", timeMode = "floating", agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"self","occurrence_id":"o","item_id":"one","starts_at":"2026-03-08T07:00:00Z","derived_at":"2026-03-08T07:00:00Z"}"""))
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                CalendarContent(AppRoute.Calendar(CalendarView.Day, "2026-03-08"), LocalDate(2026,3,8), TimeZone.of("America/New_York"), "self", listOf(row), listOf(MemberRead(false, "self", "h", "Sam Rivera", "user", "#FABD2F", "SR", "America/New_York", "manual", 1)), false, false, null, {}, {}, {})
            } } }
            for (label in listOf("Day", "Week", "Month", "Previous day", "Next day", "Today", "Refresh calendar")) {
                val node = onNodeWithContentDescription(label).assertExists().fetchSemanticsNode()
                assertTrue(node.boundsInRoot.width >= 48 && node.boundsInRoot.height >= 48)
            }
            // Reveal the bounded hour viewport, then scroll it back from work hours to the early appointment.
            val viewportY = onNodeWithTag("calendar-hours-scroll").fetchSemanticsNode().positionInRoot.y
            val entryY = onNodeWithContentDescription("Open Meds").fetchSemanticsNode().positionInRoot.y
            onNodeWithTag("calendar-scroll").performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.ScrollBy) { it(0f, viewportY) }
            onNodeWithTag("calendar-hours-scroll").performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.ScrollBy) { it(0f, entryY - viewportY) }
            onNodeWithText("Floating · 03:00", useUnmergedTree = true).assertIsDisplayed()
            onNodeWithText("Pending derivation").assertExists()
            onAllNodesWithText("SR", useUnmergedTree = true).assertCountEquals(2)
            onAllNodes(hasContentDescription("Calendar for Sam Rivera")).assertCountEquals(1)
            onAllNodes(SemanticsMatcher.keyIsDefined(androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult)).fetchSemanticsNodes().forEach { node ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                node.config[androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult].action?.invoke(layouts)
                assertTrue(layouts.none { it.didOverflowHeight || (0 until it.lineCount).any { line -> it.getLineRight(line) - it.getLineLeft(line) > it.size.width + 1f } }, "Calendar text clipped: ${layouts.map { "${it.layoutInput.text} size=${it.size} constraints=${it.layoutInput.constraints} paragraphWidth=${it.multiParagraph.width} intrinsic=${it.multiParagraph.intrinsics.maxIntrinsicWidth} width=${it.didOverflowWidth} height=${it.didOverflowHeight}" }}")
            }
            onNodeWithContentDescription("Open Meds").assertIsDisplayed()
        }
    }
    @Test fun desktopMonthAndWeekKeepDateContextAndAllDayBusyWarnings() = runDesktopComposeUiTest(width = 1200, height = 1400) {
        var route by mutableStateOf(AppRoute.Calendar(CalendarView.Month, "2026-10-07"))
        val row = itemRead("date").copy(title = "School", agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"self","occurrence_id":"o","item_id":"date","due_date":"2026-10-07","derived_at":"2026-10-06T12:00:00Z"}"""),
            conflicts = listOf(WireJson.decodeFromString<ConflictRead>("""{"member_id":"self","occurrence_id":"o","other_item_id":null,"other_title":"Busy","starts_at":"2026-10-07T12:00:00Z","ends_at":"2026-10-07T13:00:00Z","kind":"hard"}""")))
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { CalendarContent(route, LocalDate(2026,10,7), TimeZone.UTC, "self", listOf(row), emptyList(), false, false, null, { route = it }, {}, {}) } }
        onNodeWithContentDescription("Month").assertIsSelected()
        onNodeWithText("Conflict · Busy · 12:00–13:00").assertExists()
        onNodeWithText("Fixed · All day").assertExists()
        onNodeWithContentDescription("Week").performClick()
        onNodeWithContentDescription("Week").assertIsSelected()
        assertEquals("2026-10-07", route.date)
        onNodeWithContentDescription("Next week").performClick()
        assertEquals("2026-10-14", route.date)
    }
    @Test fun compareAndAmbiguousFeedbackRequireExplicitActionsInBothThemes() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 600, height = 1000) {
            var state by mutableStateOf(SettingsWriteState(SettingsWriteStatus.Conflict))
            var reapply = 0
            var retry = 0
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                Column(Modifier.verticalScroll(rememberScrollState())) { SettingsWriteFeedback(state, { retry++ }, { reapply++ }, {}) }
            } } }
            onNodeWithContentDescription("Reapply mine").performScrollTo().performClick()
            assertEquals(1, reapply)
            onNodeWithContentDescription("Discard mine").assertExists()
            runOnIdle { state = SettingsWriteState(SettingsWriteStatus.Ambiguous) }
            onNodeWithContentDescription("Retry same save").performScrollTo().performClick()
            assertEquals(1, retry)
            onNodeWithContentDescription("Discard mine").assertDoesNotExist()
        }
    }
    @Test fun todaySettingsHasOrderingRangeAndExplicitSave() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 600, height = 1400) {
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                Column(Modifier.verticalScroll(rememberScrollState())) { TodaySettingsForm(settingsRead(), false, {}) }
            } } }
            onNodeWithContentDescription("This week days").performScrollTo().performTextReplacement("14")
            onNodeWithContentDescription("Save Today settings").performScrollTo().assertIsDisplayed()
            onNodeWithContentDescription("Move Today down").performScrollTo().assertExists()
            onNodeWithContentDescription("Enable Overdue").performScrollTo().performClick()
            val save = onNodeWithContentDescription("Save Today settings").performScrollTo().fetchSemanticsNode()
            assertTrue(save.boundsInRoot.height >= 48 && save.boundsInRoot.width >= 48)
            onNodeWithContentDescription("Enable Pinned list").assertIsNotEnabled()
            onAllNodes(SemanticsMatcher.keyIsDefined(androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult)).fetchSemanticsNodes().forEach { node ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                node.config[androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult].action?.invoke(layouts)
                assertTrue(layouts.none { it.didOverflowHeight || (0 until it.lineCount).any { line -> it.getLineRight(line) - it.getLineLeft(line) > it.size.width + 1f } }, "Settings text clipped: ${layouts.map { "${it.layoutInput.text} size=${it.size} constraints=${it.layoutInput.constraints} paragraphWidth=${it.multiParagraph.width} intrinsic=${it.multiParagraph.intrinsics.maxIntrinsicWidth} width=${it.didOverflowWidth} height=${it.didOverflowHeight}" }}")
            }
        }
    }
}
