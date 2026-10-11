package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.itemRead
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.ui.EmptyState
import kotlinx.datetime.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2aCalendarTest {
    private val date = LocalDate(2026, 10, 7)
    private val members = listOf(
        MemberRead(false, "self", "h", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1),
        MemberRead(false, "other", "h", "Alex", "user", "#ABCDEF", "AX", "UTC", "manual", 1))
    private fun row(member: String = "self", from: String = "08:00", to: String = "08:01") = itemRead("shared").copy(title = "Visit",
        agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"$member","occurrence_id":"shared","item_id":"shared","starts_at":"2026-10-07T${from}:00Z","ends_at":"2026-10-07T${to}:00Z","derived_at":"2026-10-06T12:00:00Z"}"""))
    private fun grid(rows: List<ItemRead>) = calendarGrid(calendarWindow(CalendarView.Day, date, TimeZone.UTC), rows, setOf("self", "other"), TimeZone.UTC)

    @Test fun lateBlocksRemainInsideDayWithoutShrinkingTargets() {
        val entry = grid(listOf(row(from = "23:59", to = "23:59"))).days.single().timed.single()
        assertTrue(entry.startMinute + entry.displayDurationMinutes <= 24 * 60, "Drawn block overruns day end")
        assertTrue(entry.displayDurationMinutes >= CalendarGridScale().minimumDurationMinutes)
        assertEquals("23:59", entry.startLabel, "Moving drawn bounds must not change the server time label")
    }
    @Test fun sharedResolvedOccurrenceHasOneOverlayCard() {
        assertEquals(1, grid(listOf(row(), row("other"))).days.single().timed.size, "Shared occurrence duplicated")
    }
    @Test fun differentResolvedFloatingTimesAreNotCollapsed() {
        assertEquals(2, grid(listOf(row(), row("other", "09:00", "09:01"))).days.single().timed.size)
    }
    @Test fun compactCardHasVisibleMemberIdentity() = runDesktopComposeUiTest(width = 900, height = 1600) {
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { DesktopCalendarGrid(grid(listOf(row())), date, TimeZone.UTC, members, {}) } }
        onNode(hasText("SR", substring = true) and hasAnyAncestor(hasContentDescription("Open Visit")), useUnmergedTree = true).assertExists()
    }
    @Test fun compactLoadWarningIsNotAnOverlapConflict() = runDesktopComposeUiTest(width = 900, height = 1600) {
        val conflict = WireJson.decodeFromString<ConflictRead>("""{"member_id":"self","occurrence_id":"shared","other_item_id":null,"other_title":"Busy","starts_at":"2026-10-07T08:00:00Z","ends_at":"2026-10-07T08:01:00Z","kind":"load"}""")
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { DesktopCalendarGrid(grid(listOf(row().copy(conflicts = listOf(conflict)))), date, TimeZone.UTC, members, {}) } }
        onNode(hasText("Over capacity", substring = true) and hasAnyAncestor(hasContentDescription("Open Visit")), useUnmergedTree = true).assertExists()
        onNodeWithText("Conflict", substring = true, useUnmergedTree = true).assertDoesNotExist()
    }
    @Test fun lateCompactCardKeepsItsLabelAtDoubleScale() = runDesktopComposeUiTest(width = 1200, height = 1600) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            CalendarContent(AppRoute.Calendar(CalendarView.Day, date.toString(), "self"), date, TimeZone.UTC, "self",
                listOf(row(from = "23:59", to = "23:59")), members, false, false, null, {}, {}, {})
        } } }
        val entry = onNodeWithContentDescription("Open Visit")
        onNodeWithTag("calendar-hours-scroll").performScrollTo()
        onNodeWithTag("calendar-hours-scroll").performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.ScrollBy) { it(0f, 100000f) }
        entry.assertIsDisplayed()
        val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
        onNodeWithText("SR · Visit", useUnmergedTree = true).fetchSemanticsNode()
            .config[androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult].action!!.invoke(layouts)
        assertTrue(layouts.none { it.didOverflowHeight }, "Compact label clipped at double scale")
        evidence("calendar-late-desktop-double")
    }
    @Test fun noticesHaveHeadingWithoutStealingFocus() = runDesktopComposeUiTest(width = 320, height = 640) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            EmptyState("Item unavailable", "This item is not in saved data. Reconnect and open it again.")
        } } }
        onNodeWithText("Item unavailable").assert(SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading))
        evidence("notice-320-double")
    }
    @Test fun compactCalendarReselectIsNoOpAtDoubleScale() = runDesktopComposeUiTest(width = 320, height = 1200) {
        var selections = 0
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            CalendarContent(AppRoute.Calendar(CalendarView.Day, date.toString(), "self"), date, TimeZone.UTC, "self",
                listOf(row()), members, true, false, null, {}, {}, {}, selectMember = { selections++ })
        } } }
        onNodeWithContentDescription("Calendar for Sam Rivera").performScrollTo().performClick()
        assertEquals(0, selections)
        evidence("calendar-320-double")
    }
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = java.io.File("build/test-evidence/audit-bl-client2a").apply { mkdirs() }
        assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("$name.png")))
    }
}
