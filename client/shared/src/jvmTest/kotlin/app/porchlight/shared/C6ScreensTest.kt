package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*

@OptIn(ExperimentalTestApi::class)
class C6ScreensTest {
    @Test fun everyNewScreenBothThemesDoubleScaleHasNamed48DpActions() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            val screens: List<Pair<String, @Composable () -> Unit>> = listOf(
                "Choose ICS file" to { ImportContent() },
                "Add feed" to { FeedContent() },
                "Add weekly window" to { AvailabilityContent() },
                "Request suggestions" to { PlanningCardsContent() },
                "Export my data" to { ExportContent() },
                "Refresh devices" to { DeviceAdminContent() },
            )
            for ((label, content) in screens) runDesktopComposeUiTest(width = 900, height = 1800) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) { content() } } }
                onNodeWithContentDescription(label).performScrollTo().assertExists()
                val node = onNodeWithContentDescription(label).fetchSemanticsNode()
                assertTrue(node.size.height >= 48, label)
                assertTrue(node.size.width >= 48, label)
            }
        }
    }
    private inline fun <reified T> fixture(name: String): T = WireJson.decodeFromString(
        checkNotNull(javaClass.getResourceAsStream("/planning-fixtures/$name.json")).bufferedReader().use { it.readText() })
    @Test fun populatedActionsBothThemesDoubleScaleAndReadableStatus() {
        val preview = fixture<ImportSummary>("preview")
        val suggestion = fixture<SuggestionResponse>("suggestion")
        val feed = fixture<Source>("feed").copy(failureCount = 2, lastStatus = "failed")
        val device = fixture<Device>("device").copy(current = false)
        val member = MemberRead(false, feed.targetMemberId!!, feed.householdId, "Sam", "dependent", "#FABD2F", "SM", "America/New_York", "manual", 1)
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            val screens: List<Pair<List<String>, @Composable () -> Unit>> = listOf(
                listOf("Commit import", "Cancel preview") to { ImportContent(preview, "school.ics") },
                listOf("Edit feed Recorded feed", "Refresh now: Refresh feed Recorded feed", "Remove feed Recorded feed", "Feed target Sam") to { FeedContent(rows = listOf(feed), members = listOf(member), target = member.id) },
                listOf("Accept suggestion ${suggestion.id}", "Dismiss suggestion ${suggestion.id}", "Refetch suggestions") to { PlanningCardsContent(listOf(suggestion), { "Laundry" }, memberName = { "Sam" }, zone = { member.tzid }) },
                listOf("Revoke device Test device") to { DeviceAdminContent(listOf(device), { "Sam" }, zone = member.tzid) },
            )
            screens.forEachIndexed { index, (labels, content) -> runDesktopComposeUiTest(width = 900, height = 1800) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) { content() } } }
                labels.forEach { label ->
                    onNodeWithContentDescription(label).performScrollTo().assertIsEnabled()
                    val node = onNodeWithContentDescription(label).fetchSemanticsNode()
                    assertTrue(node.size.width >= 48 && node.size.height >= 48, label)
                }
                when (index) {
                    0 -> onNodeWithText("cancelled/EXDATE occurrences:", substring = true).assertExists()
                    1 -> { onNodeWithText(feed.url!!).assertExists(); onNodeWithText("Failures: 2", substring = true).assertExists() }
                    2 -> onNodeWithText("Sam ·", substring = true).assertExists()
                    3 -> onNodeWithText("Certificate expires:", substring = true).assertExists()
                }
            } }
        }
    }
    @Test fun ordinaryMemberCannotSeeFeedAdminHouseholdExportOrDeviceActions() = runTest {
        val now = kotlin.time.Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val member = fixture<Source>("feed").ownerMemberId
        val snapshot = emptySnapshot(1)
        try {
            cache.install(snapshot.copy(memberSettings = snapshot.memberSettings.copy(memberId = member),
                members = listOf(MemberRead(false, member, "01999999-0000-7000-8000-000000000006", "Sam", "user", "#FABD2F", "SM", "UTC", "manual", 1))))
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = error("No admin read may be sent: $path")
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
            val section = mutableStateOf(SettingsSection.CalendarData)
            runDesktopComposeUiTest(width = 900, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { PlanningScreen(services, section.value) } } }
                onNodeWithText("Feed admin").assertDoesNotExist()
                runOnIdle { section.value = SettingsSection.Export }
                onNodeWithContentDescription("Export my data").assertExists()
                onNodeWithContentDescription("Export household").assertDoesNotExist()
                runOnIdle { section.value = SettingsSection.Devices }
                onNodeWithText("Super admin access required.").assertExists()
                onNodeWithContentDescription("Refresh devices").assertDoesNotExist()
            }
        } finally { cache.close() }
    }

}
