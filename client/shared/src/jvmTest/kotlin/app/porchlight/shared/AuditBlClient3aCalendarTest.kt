package app.porchlight.shared

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
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
class AuditBlClient3aCalendarTest {
    @Test fun workHoursInitiallyVisibleWithoutLosingNavigationOrNightHours() = runDesktopComposeUiTest(width = 1000, height = 1200) {
        val date = LocalDate(2026, 10, 7)
        val member = MemberRead(false, "self", "h", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        fun row(id: String, from: String, to: String) = itemRead(id).copy(title = id,
            agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"self","occurrence_id":"$id","item_id":"$id","starts_at":"2026-10-07T${from}:00Z","ends_at":"2026-10-07T${to}:00Z","derived_at":"2026-10-06T12:00:00Z"}"""))
        setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            CalendarContent(AppRoute.Calendar(CalendarView.Day, date.toString(), "self"), date, TimeZone.UTC, "self",
                listOf(row("Morning", "08:00", "09:00"), row("Midnight", "00:00", "01:00"), row("Late", "23:00", "23:59")),
                listOf(member), false, false, null, {}, {}, {})
        } }
        onNodeWithContentDescription("Previous day").assertIsDisplayed()
        onNodeWithContentDescription("Open Morning").assertIsDisplayed()
        onNodeWithContentDescription("Open Midnight").assertIsNotDisplayed()
        evidence("work-hours")
        onNodeWithTag("calendar-hours-scroll").performScrollTo().performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, -100000f) }
        onNodeWithContentDescription("Open Midnight").assertIsDisplayed()
        onNodeWithTag("calendar-hours-scroll").performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 100000f) }
        onNodeWithContentDescription("Open Late").assertIsDisplayed()
        onNodeWithContentDescription("Previous day").performScrollTo().assertIsDisplayed()
    }
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = java.io.File("build/test-evidence/audit-bl-client3a").apply { mkdirs() }
        assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("$name.png")))
    }
}
