package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.files.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class C6ConnectedTest {
    @Test fun importAmbiguousCommitSurvivesRemovalAndRetriesSameBodyAndKey() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val commits = mutableListOf<Pair<JsonObject?, String?>>()
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/imports/ics", path)
                val preview = body!!["preview"]?.jsonPrimitive?.boolean == true
                if (!preview) {
                    commits += body to idempotencyKey
                    if (commits.size == 1) throw ConnectFailure()
                }
                return WireJson.parseToJsonElement("""{"created":1,"updated":0,"unchanged":0,"removed":0,"deleted":0,"skipped":0,"warnings":[],"item_ids":[],"conflicts":[],"preview":$preview,"removal_suppressed":false}""").jsonObject
            }
        }
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex","user","#FABD2F","AX","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            data.imports.afterCommit = {}
            val services = object : AppServices by FakeServices() {
                override val data = data
                override val screenSession = ScreenSession()
            }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { onResult(Result.success(PickedCalendar("school.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR"))) }
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) = error("Not used")
            }
            val visible = mutableStateOf(true)
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) {
                    if (visible.value) PlanningScreen(services, SettingsSection.CalendarData, files) else Text("Locked")
                } } }
                onNodeWithContentDescription("Choose ICS file").performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Commit import").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Commit import").performClick()
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writes("c6:CalendarData:01999999-0000-7000-8000-000000000001").state.value.retryKey != null }
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { visible.value = true }
                onNodeWithContentDescription("Retry identical request").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { commits.size == 2 && services.screenSession.writes("c6:CalendarData:01999999-0000-7000-8000-000000000001").state.value.saved }
                assertEquals(commits[0], commits[1])
                assertEquals(false, commits[0].first!!["preview"]?.jsonPrimitive?.boolean)
                onNodeWithText("Saved").assertExists()
            }
        } finally { cache.close() }
    }
    @Test fun rejectedProposalThatServerStillListsCannotBeAcceptedAgainAfterRefetch() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val row = WireJson.decodeFromString<SuggestionResponse>(checkNotNull(javaClass.getResourceAsStream("/planning-fixtures/suggestion.json")).bufferedReader().use { it.readText() })
        var reads = 0
        var current = row
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") {
                    reads++
                    return buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(current)))); put("next_cursor", JsonNull) }
                }
                throw ApiProblem.decode("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Validation failed","errors":[{"field":"proposed_start","code":"not-future","message":"Proposal no longer future"}]}""")
            }
        }
        try {
            val snapshot = emptySnapshot(1)
            cache.install(snapshot.copy(memberSettings = snapshot.memberSettings.copy(memberId = row.memberId),
                members = listOf(MemberRead(false,row.memberId,"01999999-0000-7000-8000-000000000006","Sam","user","#FABD2F","SM","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
            runDesktopComposeUiTest(width = 900, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) { PlanningScreen(services, SettingsSection.Planning) } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Accept suggestion ${row.id}").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Accept suggestion ${row.id}").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writes("c6:Planning:${row.memberId}").state.value.fieldErrors.isNotEmpty() }
                onNodeWithContentDescription("Accept suggestion ${row.id}").assertIsNotEnabled()
                onNodeWithText("Proposal expired or no longer applies.").assertExists()
                val before = reads
                onNodeWithContentDescription("Refetch suggestions").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { reads > before }
                onNodeWithContentDescription("Accept suggestion ${row.id}").assertIsNotEnabled()
                val beforeNew = reads
                runOnIdle { current = row.copy(version = row.version + 1) }
                onNodeWithContentDescription("Refetch suggestions").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { reads > beforeNew }
                onNodeWithContentDescription("Accept suggestion ${row.id}").assertIsEnabled()
                onNodeWithText("Proposal expired or no longer applies.").assertDoesNotExist()
            }
            assertTrue(services.screenSession.rejectedSuggestions.isNotEmpty())
            services.screenSession.bind(99)
            assertTrue(services.screenSession.rejectedSuggestions.isEmpty())
        } finally { cache.close() }
    }

}
