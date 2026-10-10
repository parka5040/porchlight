package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.CalendarContent
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.ui.*
import kotlinx.datetime.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class Fix2LayoutTest {
    private val members = listOf(
        MemberRead(false, "self", "h", "Member One", "user", "#FABD2F", "MO", "UTC", "manual", 1),
        MemberRead(false, "other", "h", "Member Two", "user", "#8EC07C", "MT", "UTC", "manual", 1),
    )
    private fun calendar(mobile: Boolean = false, view: CalendarView = CalendarView.Week,
        test: ComposeUiTest.() -> Unit) = runDesktopComposeUiTest(width = 1024, height = 1000) {
        var route by mutableStateOf(AppRoute.Calendar(view, "2026-10-09", "self"))
        val row = itemRead("appointment").copy(title = "Fixture appointment", agenda = WireJson.decodeFromString<AgendaRead>(
            """{"household_id":"h","member_id":"self","occurrence_id":"appointment","item_id":"appointment","starts_at":"2026-10-09T08:00:00Z","ends_at":"2026-10-09T09:00:00Z","derived_at":"2026-10-09T07:00:00Z"}"""))
        setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            CalendarContent(route, LocalDate(2026, 10, 9), TimeZone.UTC, route.memberId, listOf(row), members,
                mobile, false, null, { route = it }, {}, {})
        } }
        test()
    }

    @Test fun overlayUsesOnlyCheckedState() = calendar {
        val overlay = onNodeWithContentDescription("Overlay Member Two")
        overlay.assertIsOff().assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.Selected))
        overlay.performClick().assertIsOn()
        overlay.performClick().assertIsOff()
    }

    @Test fun memberRowsHaveOneMergedStopAndRadioGroup() = calendar(mobile = true) {
        members.forEachIndexed { index, member ->
            val row = onNodeWithContentDescription("Calendar for ${member.displayName}")
            onAllNodesWithContentDescription("Calendar for ${member.displayName}").assertCountEquals(1)
            row.assert(SemanticsMatcher.expectValue(SemanticsProperties.Selected, index == 0))
            row.assert(hasAnyAncestor(SemanticsMatcher.keyIsDefined(SemanticsProperties.SelectableGroup)))
            onNodeWithContentDescription("Member ${member.displayName}").assertDoesNotExist()
            assertTrue(row.fetchSemanticsNode().children.isEmpty(), "Member row must be a single merged accessibility stop")
        }
        onNodeWithContentDescription("Calendar for Member Two").performClick().assertIsSelected()
        onNodeWithContentDescription("Calendar for Member One").assertIsNotSelected()
    }

    @Test fun desktopAgendaKeepsDesignPendingMarkerWhenPopulated() = calendar(view = CalendarView.Agenda) {
        onNodeWithText("Fixture appointment").assertExists()
        onNodeWithText("DESIGN-PENDING: Calendar Agenda").assertExists()
    }

    @Test fun agendaNavigationNamesItsSevenDayStep() = calendar(view = CalendarView.Agenda) {
        onNodeWithContentDescription("Previous 7 days").assertExists()
        onNodeWithContentDescription("Next 7 days").performClick()
        onNodeWithText("Oct 16 – 22, 2026").assertExists()
    }

    @Test fun segmentedChoiceSupportsIntrinsicHeightParent() = runDesktopComposeUiTest(width = 360, height = 500) {
        setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            Row(Modifier.height(IntrinsicSize.Min)) {
                SegmentedChoice(listOf(ScopeOption("one", "One"), ScopeOption("two", "Two")), "one", {})
            }
        } }
        onNodeWithContentDescription("One").assertIsDisplayed().assertIsSelected()
        onNodeWithContentDescription("Two").assertIsDisplayed().assertIsNotSelected()
    }

    @Test fun dragIntoLowerHalfOfEmptySideBySideColumnMovesCard() = runDesktopComposeUiTest(width = 1600, height = 1000) {
        val row = itemRead("card").copy(title = "Fixture card", capabilities = listOf("view", "change_status"),
            occurrence = occurrenceRead("occ-card", "card").copy(status = "backlog"))
        var lastDrop: BoardDrop? = null
        setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            BoardContent(boardColumns(listOf(row), null, "self"), emptyList(), {}, { _, drop -> lastDrop = drop })
        } }
        val card = onNodeWithTag("board-card-occ-card")
        val origin = card.fetchSemanticsNode().boundsInRoot
        val empty = onNodeWithTag("board-heading-todo").fetchSemanticsNode().boundsInRoot
        val viewport = onRoot().fetchSemanticsNode().boundsInRoot
        val release = Offset(empty.center.x, viewport.top + viewport.height * .75f)
        assertTrue(release.y > viewport.center.y)
        card.performTouchInput {
            down(Offset(origin.width / 2, origin.height / 8))
            advanceEventTime(900)
            moveTo(release - origin.topLeft)
            up()
        }
        assertEquals(BoardDrop("todo", 0), lastDrop)
    }
}
