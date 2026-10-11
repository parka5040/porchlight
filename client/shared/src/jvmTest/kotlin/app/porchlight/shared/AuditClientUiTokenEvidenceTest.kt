package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.backups.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.screens.inbox.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.test.Test
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiTokenEvidenceTest {
    @Test fun tokenLayoutsAtNarrowAndWideDoubleFont() = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val row = itemRead("item").copy(title = "Laundry", notes = "Fixture notes", occurrence = occurrenceRead("occ", "item"))
        val cache = memoryCache { now }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        val backup = object : DesktopBackups {
            override val state = MutableStateFlow(BackupView(allowed = true, notice = "Fixture backup status"))
            override suspend fun pull() {}
        }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row), members = listOf(member))) }
            val screens: List<Pair<String, @Composable () -> Unit>> = listOf(
                "today" to { TodayContent(listOf(TodayGroup(TodaySection.today, listOf(row))), 7, listOf(member), {}, {}) },
                "inbox" to { InboxContent(listOf(InboxGroup("Today", listOf(notificationFixture()))), {}, {}, {}, {}) },
                "editor" to { EditorForm(EditorDraft.from(row), {}, emptyMap(), listOf(member), false, EditorScope.All, {}) },
                "detail" to { Column(Modifier.verticalScroll(rememberScrollState())) { ItemDetailContent(row, listOf(member), {}, {}, {}, {}, {}) } },
                "colors" to { ColorChoices("#FABD2F", false, {}) },
                "today-settings" to { Column(Modifier.verticalScroll(rememberScrollState())) { TodaySettingsForm(emptySnapshot(1).memberSettings, false, {}) } },
                "planning" to { PlanningCardsContent() },
                "notification-preferences" to { NotificationPreferences(services) },
                "backups" to { BackupPanel(backup) },
                "calendar-day" to { CalendarContent(AppRoute.Calendar(CalendarView.Day, "2026-10-11"), LocalDate(2026, 10, 11), TimeZone.UTC, "self", listOf(row), listOf(member), false, false, null, {}, {}, {}) },
                "calendar-month" to { CalendarContent(AppRoute.Calendar(CalendarView.Month, "2026-10-11"), LocalDate(2026, 10, 11), TimeZone.UTC, "self", listOf(row), listOf(member), false, false, null, {}, {}, {}) },
                "shell" to { PorchlightApp(FakeServices()) }
            )
            for (width in listOf(320, 1200)) for ((name, content) in screens) {
                runDesktopComposeUiTest(width = width, height = 1600) {
                    setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) { Surface(Modifier.fillMaxSize()) { content() } } } }
                    waitForIdle()
                    auditEvidence("tokens-$name-$width")
                }
            }
        } finally { services.screenSession.writeScope.cancel(); cache.close() }
    }
}
