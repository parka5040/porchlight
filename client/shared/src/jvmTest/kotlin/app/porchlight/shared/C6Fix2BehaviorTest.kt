package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.semantics.SemanticsActions
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
class C6Fix2BehaviorTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private inline fun <reified T> fixture(name: String): T = WireJson.decodeFromString(
        checkNotNull(javaClass.getResourceAsStream("/planning-fixtures/$name.json")).bufferedReader().use { it.readText() })
    private suspend fun install(cache: ReadCache, member: String, admin: Boolean = false) {
        val snapshot = emptySnapshot(1)
        cache.install(snapshot.copy(memberSettings = snapshot.memberSettings.copy(memberId = member), members = listOf(
            MemberRead(false, member, "01999999-0000-7000-8000-000000000006", "Alex", if (admin) "super_admin" else "user", "#FABD2F", "AX", "UTC", "manual", 1),
            MemberRead(false, "01999999-0000-7000-8000-000000000002", "01999999-0000-7000-8000-000000000006", "Sam", "dependent", "#FABD2F", "SM", "UTC", "manual", 1))))
    }
    private fun services(data: HouseholdData) = object : AppServices by FakeServices() {
        override val data = data
        override val screenSession = ScreenSession()
    }

    @Test fun queuedFeedThenDoubleTapLongFilenameImportShowsSavedAndDependentDefaultsHousehold() = runTest {
        val cache = memoryCache { now }
        val feed = fixture<Source>("feed")
        var callback: ((Result<PickedCalendar?>) -> Unit)? = null
        var picks = 0
        var previews = 0
        var commits = 0
        val name = "x".repeat(220) + ".ics"
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when {
                method == "GET" -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(feed)))); put("next_cursor", JsonNull) }
                path.endsWith("/refresh") -> WireJson.encodeToJsonElement(feed).jsonObject
                path == "/v1/imports/ics" -> {
                    assertTrue(body!!["scope"]!!.jsonPrimitive.content.length <= 200)
                    val preview = body["preview"]!!.jsonPrimitive.boolean
                    if (preview) previews++ else commits++
                    WireJson.parseToJsonElement("""{"created":1,"updated":0,"unchanged":0,"removed":0,"deleted":0,"skipped":0,"warnings":[],"item_ids":[],"conflicts":[],"preview":$preview,"removal_suppressed":false}""").jsonObject
                }
                else -> error("Unexpected request")
            }
        }
        try {
            install(cache, feed.ownerMemberId, admin = true)
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            data.imports.afterCommit = {}
            val services = services(data)
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { picks++; callback = onResult }
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) = error("Not used")
            }
            runDesktopComposeUiTest(width = 900, height = 2200) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { PlanningScreen(services, SettingsSection.CalendarData, files) } } }
                onNodeWithContentDescription("Feed admin").performClick()
                onNodeWithContentDescription("Feed target Sam").performClick()
                onNodeWithContentDescription("Household visible: Feed visibility household").assertIsSelected()
                onNodeWithContentDescription("Feed target Alex").performClick()
                onNodeWithContentDescription("Feed visibility private").assertIsSelected()
                onNodeWithContentDescription("Refresh now: Refresh feed ${feed.name}").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writes("c6:CalendarData:${feed.ownerMemberId}").state.value.saved }
                onNodeWithText("Refresh queued; polling has not completed.").assertExists()
                onNodeWithContentDescription("ICS upload").performClick()
                val click = onNodeWithContentDescription("Choose ICS file").fetchSemanticsNode().config[SemanticsActions.OnClick].action!!
                runOnIdle { click(); click() } // same callback/frame, before recomposition disables the button
                assertEquals(1, picks)
                runOnIdle { callback!!(Result.success(PickedCalendar(name, "BEGIN:VCALENDAR\r\nEND:VCALENDAR"))) }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Commit import").fetchSemanticsNodes().isNotEmpty() }
                assertEquals(1, previews)
                onNodeWithContentDescription("Commit import").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { commits == 1 }
                onNodeWithText("Saved").assertExists()
                onNodeWithText("Refresh queued; polling has not completed.").assertDoesNotExist()
            }
        } finally { cache.close() }
    }

    @Test fun planningRun422KeepsItsOwnServerDetail() = runTest {
        val cache = memoryCache { now }
        val row = fixture<SuggestionResponse>("suggestion")
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") return buildJsonObject { put("items", buildJsonArray {}); put("next_cursor", JsonNull) }
                throw ApiProblem.decode("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Validation failed","detail":"Add an estimate before requesting suggestions."}""")
            }
        }
        try {
            install(cache, row.memberId)
            val services = services(HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now }))
            runDesktopComposeUiTest(width = 900, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { PlanningScreen(services, SettingsSection.Planning) } } }
                onNodeWithContentDescription("Request suggestions").performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Add an estimate before requesting suggestions.").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Proposal expired or no longer applies; suggestions refetched. Use Refetch suggestions if offline.").assertDoesNotExist()
            }
        } finally { cache.close() }
    }

    @Test fun expiredProposalsStillAllowDismissInBothPanels() = runTest {
        for (calendarPanel in listOf(false, true)) {
            val cache = memoryCache { now }
            val row = fixture<SuggestionResponse>("suggestion")
            var dismissed = 0
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    if (method == "GET") return buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(row)))); put("next_cursor", JsonNull) }
                    assertTrue(path.endsWith("/dismiss"))
                    assertEquals(row.version.toLong(), version)
                    dismissed++
                    return WireJson.encodeToJsonElement(row.copy(status = SuggestionStatus.dismissed)).jsonObject
                }
            }
            try {
                install(cache, row.memberId)
                cache.putSuggestion(row)
                val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
                data.planning.afterAccept = {}
                val services = services(data)
                runDesktopComposeUiTest(width = 900, height = 1800) {
                    setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                        if (calendarPanel) { val view by cache.view.collectAsState(); Column { CalendarPlanningPanel(services, view) {} } }
                        else PlanningScreen(services, SettingsSection.Planning)
                    } } }
                    waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Accept suggestion ${row.id}").fetchSemanticsNodes().isNotEmpty() }
                    runOnIdle { services.screenSession.rejectSuggestion(row) }
                    onNodeWithContentDescription("Accept suggestion ${row.id}").assertIsNotEnabled()
                    onNodeWithContentDescription("Dismiss suggestion ${row.id}").assertIsEnabled().performClick()
                    waitUntil(timeoutMillis = 10_000) { dismissed == 1 }
                    onNodeWithContentDescription("Accept suggestion ${row.id}").assertDoesNotExist()
                }
            } finally { cache.close() }
        }
    }
}
