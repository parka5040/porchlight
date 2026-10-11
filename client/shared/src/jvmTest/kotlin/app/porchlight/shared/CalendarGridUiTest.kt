package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.semantics.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.itemRead
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.WireJson
import kotlinx.datetime.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class CalendarGridUiTest {
    private val members = listOf(MemberRead(false, "self", "h", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1),
        MemberRead(false, "alex", "h", "Alex", "user", "#ABCDEF", "AX", "UTC", "manual", 1))
    private fun row(id: String, member: String, from: String, to: String) = itemRead(id).copy(title = id,
        agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"$member","occurrence_id":"$id","item_id":"$id","starts_at":"$from","ends_at":"$to","derived_at":"2026-10-06T12:00:00Z"}"""))
    @Test fun overlayGridPositionsOverlapSideBySideAtDoubleScaleBothThemes() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 1200, height = 1600) {
            var route by mutableStateOf(AppRoute.Calendar(CalendarView.Week, "2026-10-07", "self"))
            val rows = listOf(row("First", "self", "2026-10-04T08:00:00Z", "2026-10-04T10:00:00Z"),
                row("Second", "alex", "2026-10-04T09:00:00Z", "2026-10-04T11:00:00Z"),
                row("Short", "self", "2026-10-04T12:00:00Z", "2026-10-04T12:01:00Z"))
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                CalendarContent(route, LocalDate(2026,10,7), TimeZone.UTC, "self", rows, members, false, false, null, { route = it }, {}, {})
            } } }
            onNodeWithContentDescription("Open Second").assertDoesNotExist()
            onNodeWithContentDescription("Overlay Alex").assertIsOff().performClick().assertIsOn()
            val first = onNodeWithContentDescription("Open First").fetchSemanticsNode()
            val second = onNodeWithContentDescription("Open Second").fetchSemanticsNode()
            assertTrue(second.positionInRoot.x >= first.positionInRoot.x + first.size.width)
            assertEquals(320f, second.positionInRoot.y - first.positionInRoot.y, 1f)
            assertEquals(first.size, second.size)
            assertTrue(second.config[SemanticsProperties.StateDescription].startsWith("Alex"))
            onAllNodesWithText("AX", useUnmergedTree = true).assertCountEquals(2) // server initials, not recomputed "A"
            onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach { node ->
                assertTrue(node.config.contains(SemanticsProperties.ContentDescription), "Missing description")
                assertTrue(node.size.width >= 48 && node.size.height >= 48, "Small target: ${node.size}")
            }
            onNodeWithContentDescription("Next week").performClick()
            assertEquals("alex", route.overlay)
        }
    }
    @Test fun dstWeeksRenderEachDaysOwnHourAxis() {
        for ((zoneId, transition) in listOf("America/New_York" to "2026-03-08", "America/New_York" to "2026-11-01",
            "Europe/Berlin" to "2026-03-29", "Europe/Berlin" to "2026-10-25")) runDesktopComposeUiTest(width = 1200, height = 1600) {
            val zone = TimeZone.of(zoneId)
            val date = LocalDate.parse(transition)
            val window = calendarWindow(CalendarView.Week, date, zone)
            val rows = window.dates.map { day -> row(day.toString(), "self",
                LocalDateTime(day, LocalTime(9, 0)).toInstant(zone).toString(),
                LocalDateTime(day, LocalTime(10, 0)).toInstant(zone).toString()) }
            val grid = calendarGrid(window, rows, setOf("self"), zone)
            setContent { PorchlightTheme(ThemeMode.Daylight, false) { DesktopCalendarGrid(grid, date, zone, members, {}) } }
            grid.days.asReversed().forEach { day ->
                val tick = onNode(hasText("09:00", substring = true) and !hasClickAction() and hasAnyAncestor(hasTestTag("calendar-day-${day.date}"))).fetchSemanticsNode()
                val block = onNodeWithContentDescription("Open ${day.date}").fetchSemanticsNode()
                assertEquals(tick.positionInRoot.y, block.positionInRoot.y, 4f, "$zoneId ${day.date}")
                assertTrue(block.positionInRoot.x >= tick.positionInRoot.x + tick.size.width)
            }
        }
    }
    @Test fun minimumHeightBlocksDrawSideBySideAtCurrentScaleBothThemes() {
        for (theme in listOf(ThemeMode.Daylight, ThemeMode.Evening)) for (fontScale in listOf(1f, 2f))
            runDesktopComposeUiTest(width = 1200, height = 1600) {
                val date = LocalDate(2026,10,7)
                val rows = listOf(row("Short", "self", "2026-10-07T08:00:00Z", "2026-10-07T08:05:00Z"),
                    row("Close", "self", "2026-10-07T08:08:00Z", "2026-10-07T08:40:00Z"))
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, fontScale)) { PorchlightTheme(theme, false) {
                    CalendarContent(AppRoute.Calendar(CalendarView.Day, date.toString(), "self"), date, TimeZone.UTC,
                        "self", rows, members, false, false, null, {}, {}, {})
                } } }
                val short = onNodeWithContentDescription("Open Short").fetchSemanticsNode()
                val close = onNodeWithContentDescription("Open Close").fetchSemanticsNode()
                assertTrue(close.positionInRoot.y < short.positionInRoot.y + short.size.height)
                assertTrue(close.positionInRoot.x >= short.positionInRoot.x + short.size.width)
                assertTrue(short.size.height >= 48 && short.size.width >= 48)
                assertEquals((52 * fontScale).toInt() - 4, short.size.height) // Keep the label readable at the requested scale.
            }
    }
    @Test fun narrowMobileAgendaAtDoubleScaleHasNoEmptyDaysOrSecretConflictText() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 360, height = 1400) {
            val row = row("Dentist", "self", "2026-10-07T12:00:00Z", "2026-10-07T13:00:00Z").copy(conflicts = listOf(
                WireJson.decodeFromString<ConflictRead>("""{"member_id":"self","occurrence_id":"Dentist","other_item_id":null,"other_title":"Private title","starts_at":"2026-10-07T12:00:00Z","ends_at":"2026-10-07T13:00:00Z","kind":"hard"}""")))
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                CalendarContent(AppRoute.Calendar(CalendarView.Agenda, "2026-10-07"), LocalDate(2026,10,7), TimeZone.UTC, "self", listOf(row), members, true, false, null, {}, {}, {})
            } } }
            onNodeWithText("Nothing scheduled").assertDoesNotExist()
            onNodeWithText("Private title", substring = true).assertDoesNotExist()
            onNodeWithText("Conflict · Busy · 12:00–13:00").performScrollTo().assertIsDisplayed()
            onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach { node ->
                assertTrue(node.config.contains(SemanticsProperties.ContentDescription))
                assertTrue(node.size.width >= 48 && node.size.height >= 48)
            }
            onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsActions.GetTextLayoutResult)).fetchSemanticsNodes().forEach { node ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                node.config[SemanticsActions.GetTextLayoutResult].action?.invoke(layouts)
                assertTrue(layouts.none { it.didOverflowHeight || (0 until it.lineCount).any { line -> it.getLineRight(line) - it.getLineLeft(line) > it.size.width + 1f } })
            }
        }
    }
}
