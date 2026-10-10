package app.porchlight.shared

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class CalendarNavigationTest {
    @Test fun memberZoneSurvivesNextPreviousTodayAndViewChangesThroughNavHost() {
        val directory = Files.createTempDirectory(File(checkNotNull(System.getProperty("porchlight.test.dir"))).toPath(), "calendar-nav-").toFile()
        val fixture = FakeServices()
        val cache = ReadCache(openDesktopCache(File(directory, "cache.db")), fixture.clock::now)
        val data = HouseholdData(cache, GeneratedCall({ error("offline fixture") }, { error("offline fixture") }), fixture.clock::now)
        try {
            runTest { cache.install(emptySnapshot(1).copy(members = listOf(
                MemberRead(false, "self", "h", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1),
                MemberRead(false, "alex", "h", "Alex", "user", "#8EC07C", "A", "America/Los_Angeles", "manual", 1)))) }
            val services = object : AppServices {
                override val settings = fixture.settings
                override val platform = fixture.platform
                override val clock = fixture.clock
                override val data = data
            }
            runDesktopComposeUiTest(width = 1200, height = 1000) {
                var saved: AppRoute = AppRoute.Today
                setContent { PorchlightTheme(ThemeMode.Evening, false) {
                    ShellNavigation(AppRoute.Calendar(CalendarView.Day, "2026-10-05"), DeviceSettings(), fixture.settings, { saved = it }, services = services)
                } }
                waitUntil { onAllNodesWithContentDescription("Calendar for Alex").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Calendar for Alex").performClick()
                onNodeWithText("Mon, Oct 5, 2026").assertExists()
                onNodeWithText("Los Angeles time").assertExists()
                for ((control, period) in listOf("Next day" to "Tue, Oct 6, 2026", "Previous day" to "Mon, Oct 5, 2026", "Today" to "Mon, Oct 5, 2026", "Week" to "Oct 4 – 10, 2026", "Day" to "Mon, Oct 5, 2026")) {
                    onNodeWithContentDescription(control).performClick()
                    onNodeWithText(period).assertExists()
                    onNodeWithText("Los Angeles time").assertExists()
                    onNodeWithContentDescription("Calendar for Alex").assertIsSelected()
                }
                waitUntil { (saved as? AppRoute.Calendar)?.view == CalendarView.Day }
                assertEquals("alex", (saved as AppRoute.Calendar).memberId)
                assertEquals(saved, RouteCodec.decode(RouteCodec.encode(saved)))
            }
        } finally { data.close(); directory.deleteRecursively() }
    }
}
