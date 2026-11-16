package app.porchlight.shared

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import org.junit.Test
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
import kotlin.test.*
import kotlin.time.Instant

/** Real routed screens, including the rail/detail shell; all dimensions below are harness sizes. */
@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiEvidenceTest {
    @Test fun phoneAndExpandedEvidenceAtBothFontScales() {
        for ((width, scale) in listOf(320 to 1f, 320 to 2f, 1024 to 2f)) {
            for (screen in listOf("today", "lists", "board", "calendar", "inbox")) render(screen, width, scale)
        }
    }
    private fun render(screen: String, width: Int, scale: Float) {
        val now = Instant.parse("2026-10-07T12:00:00Z")
        val member = MemberRead(false, "self", "household", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val occurrence = occurrenceRead("occ-fixture", "fixture").copy(dueDate = kotlinx.datetime.LocalDate(2026, 10, 7))
        val item = itemRead("fixture").copy(title = "Review household supplies", capabilities = listOf("view", "change_status"), occurrence = occurrence)
        val agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"household","member_id":"self","occurrence_id":"occ-fixture","item_id":"fixture","starts_at":"2026-10-07T14:00:00Z","ends_at":"2026-10-07T15:00:00Z","derived_at":"2026-10-07T12:00:00Z"}""")
        val list = listFixture().copy(propertyEntries = listOf(entryFixture("a", "a0").copy(label = "Household supplies"), entryFixture("b", "a1").copy(label = "Seasonal checklist")))
        val snapshot = emptySnapshot(1).copy(members = listOf(member), items = listOf(item), occurrences = listOf(occurrence),
            lists = listOf(list), notifications = listOf(notificationFixture()),
            memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.today), SettingsScope.household, 7)))
        val cache = memoryCache { now }
        runBlocking { cache.install(snapshot); cache.replaceBoard(listOf(item)); cache.replaceAgenda(listOf(item.copy(agenda = agenda))) }
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            val body = when {
                request.url.encodedPath == "/v1/board" -> """{"items":[${WireJson.encodeToString(ItemRead.serializer(), item)}],"next_cursor":null}"""
                request.url.encodedPath == "/v1/agenda" -> """{"items":[${WireJson.encodeToString(ItemRead.serializer(), item.copy(agenda = agenda))}],"next_cursor":null}"""
                request.url.encodedPath == "/v1/lists" -> """{"items":[${WireJson.encodeToString(ListResponse.serializer(), list)}],"next_cursor":null}"""
                request.url.encodedPath.startsWith("/v1/lists/") -> WireJson.encodeToString(ListResponse.serializer(), list)
                else -> """{"items":[],"next_cursor":null}"""
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = ScreenSession()
            override val clock = AppClock { now }
            override val platform = object : PlatformInfo {
                override val kind = if (width == 320) PlatformKind.Android else PlatformKind.Desktop
                override val systemDark = MutableStateFlow(false)
            }
        }
        val route = when (screen) {
            "today" -> AppRoute.Today
            "lists" -> AppRoute.Lists(list.id)
            "board" -> AppRoute.Board()
            "calendar" -> AppRoute.Calendar(CalendarView.Agenda, "2026-10-07", "self")
            else -> AppRoute.Inbox
        }
        try {
            runDesktopComposeUiTest(width = width, height = 1000) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale), LocalScreenServices provides services) {
                    PorchlightTheme(ThemeMode.Daylight) { Surface(Modifier.fillMaxSize()) {
                        ShellNavigation(route, DeviceSettings(themeMode = ThemeMode.Daylight), services.settings, {}, services = services)
                    } }
                } }
                waitUntil { onAllNodesWithTag("shell-content").fetchSemanticsNodes().isNotEmpty() }
                waitForIdle()
                evidence("$screen-${width}dp-${scale.toInt()}x")
                if (screen == "lists") {
                    onNodeWithTag("lists-scroll").performScrollToNode(hasText("Household supplies"))
                    onNodeWithText("Household supplies").assertIsDisplayed()
                    evidence("$screen-rows-${width}dp-${scale.toInt()}x")
                }
                if (screen == "calendar") {
                    onNodeWithTag("calendar-scroll").performScrollToNode(hasText(item.title))
                    onNodeWithText(item.title).assertIsDisplayed()
                    evidence("$screen-rows-${width}dp-${scale.toInt()}x")
                }
            }
        } finally { services.screenSession.writeScope.cancel(); data.close(); api.close(); cache.close() }
    }
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = File("build/test-evidence/prerollout-client-ui").apply { mkdirs() }
        assertTrue(ImageIO.write(image, "png", directory.resolve("$name.png")))
    }
}
