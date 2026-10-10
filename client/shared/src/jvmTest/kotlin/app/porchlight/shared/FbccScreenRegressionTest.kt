package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

/** fbcc (Batch C) client correctness regressions: c3a-rr1 N1–N3 and m7c-astra C6-M2. */
@OptIn(ExperimentalTestApi::class)
class FbccScreenRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun services(data: HouseholdData, clock: AppClock = AppClock { now }, resumes: kotlinx.coroutines.flow.StateFlow<Long>? = null): AppServices = object : AppServices by FakeServices() {
        override val data = data
        override val clock = clock
        override val screenSession = ScreenSession()
        override val systemResumes = resumes
    }
    private val json = headersOf(HttpHeaders.ContentType, "application/json")

    // c3a-rr1 N3: a save that completes after the editor left the screen must not leave its draft/baseline behind.
    @Test fun offScreenSaveClearsItsDraftSoTheNextEditSendsOnlyNewChanges() = runTest {
        val cache = memoryCache { now }
        val patches = mutableListOf<JsonObject>()
        val versions = mutableListOf<String?>()
        val reply = CompletableDeferred<Unit>()
        var server = itemResponse("a").copy(title = "Laundry", version = 1)
        val api = transport { req ->
            if (req.method == HttpMethod.Patch) {
                patches += WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                versions += req.headers["If-Match"]
                if (patches.size == 1) {
                    reply.await()
                    server = server.copy(title = "Mine", version = 2)
                } else server = server.copy(notes = "Note", version = server.version + 1)
            }
            respond(WireJson.encodeToString(ItemResponse.serializer(), server), headers = json)
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.items.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry")), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
            val services = services(data)
            val visible = mutableStateOf(true)
            runDesktopComposeUiTest(width = 650, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (visible.value) ItemScreen("a") else Text("Away")
                } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performClick()
                onNodeWithContentDescription("Title").performTextReplacement("Mine")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil(timeoutMillis = 10_000) { patches.size == 1 }
                // The user navigates away before the reply; the application-scoped write completes off-screen.
                runOnIdle { visible.value = false }
                onNodeWithText("Away").assertExists()
                runOnIdle { reply.complete(Unit) }
                waitUntil(timeoutMillis = 10_000) { services.screenSession!!.writes("item:a").state.value.saved }
                runOnIdle {
                    assertNull(services.screenSession!!.drafts["a"], "saved draft survived an off-screen completion")
                    assertNull(services.screenSession!!.drafts["a:baseline"], "stale baseline survived an off-screen completion")
                }
                // Another member renames the item meanwhile.
                runOnIdle { server = server.copy(title = "Theirs", version = 3); visible.value = true }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Theirs").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Theirs").performClick()
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Note")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil(timeoutMillis = 10_000) { patches.size == 2 }
            }
            assertEquals(buildJsonObject { put("title", "Mine") }, patches[0])
            assertEquals(buildJsonObject { put("notes", "Note") }, patches[1], "the already-saved title must not be resent over the concurrent rename")
            assertEquals(listOf<String?>("1", "3"), versions)
            services.screenSession!!.writeScope.cancel()
        } finally { api.close(); cache.close() }
    }

    // c3a-rr1 N1: every 422 field error is visible while editing, either at its control or in the form's error list.
    @Test fun editorShowsEveryServerFieldErrorIncludingThoseWithoutAControl() {
        val draft = EditorDraft(title = "Laundry", tzid = "UTC")
        val errors = linkedMapOf(
            "title" to "Title is too long",
            "occurrence_id" to "Occurrence no longer exists",
            "override" to "Override is not allowed",
            "missed_policy" to "Unknown missed policy",
            "assignees" to "Assignee left the household",
            "viewers" to "Viewer left the household",
            "owner_id" to "Owner is invalid",
            "due_at" to "Due instant is invalid", // a todo shows no due-instant control
            "seq" to "Sequence is stale",
        )
        for ((scope, creating) in listOf(EditorScope.All to false, EditorScope.ThisOccurrence to false, EditorScope.All to true)) runDesktopComposeUiTest(width = 700, height = 4000) {
            setContent { PorchlightTheme(ThemeMode.Daylight, false) {
                EditorForm(draft, {}, errors, emptyList(), creating, scope, {}, scopes = listOf(EditorScope.All, EditorScope.ThisOccurrence))
            } }
            for ((field, message) in errors) {
                val shown = onAllNodesWithText(message, substring = true).fetchSemanticsNodes().size
                assertEquals(1, shown, "$scope creating=$creating: $field error shown $shown times")
            }
        }
    }

    // c3a-rr1 N1, end to end: a 422 for a field the editor has no control for is not swallowed.
    @Test fun editingSaveRejectedForAnUncontrolledFieldTellsTheUserWhy() = runTest {
        val cache = memoryCache { now }
        val api = transport { req ->
            if (req.method == HttpMethod.Patch) respond(buildJsonObject {
                put("type", "urn:porchlight:problem:validation"); put("status", 422); put("title", "Invalid")
                put("errors", buildJsonArray { add(buildJsonObject { put("field", "occurrence_id"); put("code", "invalid-reference"); put("message", "Occurrence was split by another edit") }) })
            }.toString(), HttpStatusCode.UnprocessableEntity, headersOf(HttpHeaders.ContentType, "application/problem+json"))
            else respond(WireJson.encodeToString(ItemResponse.serializer(), itemResponse("a").copy(title = "Laundry")), headers = json)
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.items.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry")), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
            val services = services(data)
            runDesktopComposeUiTest(width = 650, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("a") } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performClick()
                onNodeWithContentDescription("Title").performTextReplacement("Renamed")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil(timeoutMillis = 10_000) { services.screenSession!!.writes("item:a").state.value.fieldErrors.isNotEmpty() }
                onNodeWithText("Occurrence was split by another edit", substring = true).assertExists()
            }
            services.screenSession!!.writeScope.cancel()
        } finally { api.close(); cache.close() }
    }

    // c3a-rr1 N2: monotonic delays stall while the device sleeps; Today re-reads the wall clock when it is shown again.
    @Test fun todayMovesToTheNewDayWhenResumedAfterSleepingPastMidnight() = runTest {
        val cache = memoryCache { now }
        var wall = Instant.parse("2026-10-06T23:30:00Z")
        val api = transport { respond("", HttpStatusCode.ServiceUnavailable) }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { wall })
            val row = itemRead("a").copy(title = "Laundry", capabilities = listOf("view", "complete"))
            cache.install(emptySnapshot(1).copy(items = listOf(row), occurrences = listOf(occurrenceRead("occ", "a").copy(dueDate = kotlinx.datetime.LocalDate.parse("2026-10-07"))),
                memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.today), SettingsScope.mine, 7))))
            val services = services(data, AppClock { wall })
            val owner = object : LifecycleOwner {
                val registry = LifecycleRegistry.createUnsafe(this).apply { currentState = Lifecycle.State.RESUMED }
                override val lifecycle: Lifecycle get() = registry
            }
            runDesktopComposeUiTest(width = 600, height = 1000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalLifecycleOwner provides owner) {
                    PorchlightTheme(ThemeMode.Daylight, false) { TodayScreen() }
                } }
                onNodeWithText("Nothing here.").assertExists()
                onNodeWithContentDescription("Open Laundry").assertDoesNotExist()
                // Device sleeps overnight: the activity stops, wall time jumps, monotonic time does not.
                runOnIdle { owner.registry.currentState = Lifecycle.State.CREATED; wall = Instant.parse("2026-10-07T08:00:00Z") }
                runOnIdle { owner.registry.currentState = Lifecycle.State.RESUMED }
                waitUntil(timeoutMillis = 5_000) { onAllNodesWithContentDescription("Open Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Nothing here.").assertDoesNotExist()
            }
        } finally { api.close(); cache.close() }
    }

    private suspend fun todayFixture(wall: () -> Instant): Pair<ReadCache, HouseholdData> {
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { "session" }), wall)
        val row = itemRead("a").copy(title = "Laundry", capabilities = listOf("view", "complete"))
        cache.install(emptySnapshot(1).copy(items = listOf(row), occurrences = listOf(occurrenceRead("occ", "a").copy(dueDate = kotlinx.datetime.LocalDate.parse("2026-10-07"))),
            memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.today), SettingsScope.mine, 7))))
        return cache to data
    }

    // fbcc fix 1: a desktop wakes with the window still open and focused, so no lifecycle event arrives; the existing
    // logind PrepareForSleep(false) signal (AppServices.systemResumes) makes Today re-read the wall clock.
    @Test fun todayMovesToTheNewDayOnASystemResumeSignalWithoutALifecycleChange() = runTest {
        var wall = Instant.parse("2026-10-06T23:30:00Z")
        val (cache, data) = todayFixture { wall }
        val resumes = kotlinx.coroutines.flow.MutableStateFlow(0L)
        try {
            val services = services(data, AppClock { wall }, resumes)
            runDesktopComposeUiTest(width = 600, height = 1000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { TodayScreen() } } }
                onNodeWithText("Nothing here.").assertExists()
                // Overnight suspend: wall time jumps, monotonic time (and the lifecycle) does not.
                runOnIdle { wall = Instant.parse("2026-10-07T08:00:00Z"); resumes.value = 1 }
                waitUntil(timeoutMillis = 5_000) { onAllNodesWithContentDescription("Open Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Nothing here.").assertDoesNotExist()
            }
        } finally { data.close(); cache.close() }
    }

    // fbcc fix 1 fallback: without any wake signal, a visible Today re-checks the wall clock at least once a minute.
    @Test fun todayRechecksTheWallClockWithinAMinuteWhenNoWakeSignalArrives() = runTest {
        var wall = Instant.parse("2026-10-06T23:30:00Z")
        val (cache, data) = todayFixture { wall }
        try {
            val services = services(data, AppClock { wall })
            runDesktopComposeUiTest(width = 600, height = 1000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { TodayScreen() } } }
                onNodeWithText("Nothing here.").assertExists()
                runOnIdle { wall = Instant.parse("2026-10-07T08:00:00Z") }
                // The pre-sleep midnight timer still has ~30 minutes of monotonic time left; one minute must be enough.
                mainClock.advanceTimeBy(61_000)
                waitUntil(timeoutMillis = 5_000) { onAllNodesWithContentDescription("Open Laundry").fetchSemanticsNodes().isNotEmpty() }
            }
        } finally { data.close(); cache.close() }
    }

    // m7c-astra C6-M2: the reply to an availability save must not overwrite edits made while it was in flight.
    @Test fun availabilityEditsMadeDuringASaveSurviveItsReply() = runTest {
        val member = "01999999-0000-7000-8000-000000000001"
        val cache = memoryCache { now }
        val puts = mutableListOf<Pair<Long?, JsonObject>>()
        val reply = CompletableDeferred<Unit>()
        var server = buildJsonObject {
            put("member_id", member); put("version", 1); put("conflicts", buildJsonArray {})
            put("windows", buildJsonArray { add(buildJsonObject { put("weekday", 0); put("start", "09:00:00"); put("end", "17:00:00") }) })
        }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "PUT") {
                    puts += version to body!!
                    if (puts.size == 1) reply.await()
                    server = JsonObject(server + mapOf("version" to JsonPrimitive(server["version"]!!.jsonPrimitive.int + 1), "windows" to body["windows"]!!))
                }
                return server
            }
        }
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member),
                members = listOf(MemberRead(false, member, "01999999-0000-7000-8000-000000000006", "Alex", "adult", "#FABD2F", "AX", "UTC", "manual", 1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
            val end = hasText("Window 1 end HH:mm:ss") and hasSetTextAction()
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    PlanningScreen(services, SettingsSection.Availability, null)
                } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodes(end and hasText("17:00:00", substring = true)).fetchSemanticsNodes().isNotEmpty() }
                onNode(end).performTextReplacement("17:30:00")
                onNodeWithContentDescription("Save availability").performClick()
                waitUntil(timeoutMillis = 10_000) { puts.size == 1 }
                // Still saving on a slow connection: the user keeps editing.
                onNode(end).performTextReplacement("18:00:00")
                runOnIdle { reply.complete(Unit) }
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writes("c6:Availability:$member").state.value.saved }
                waitForIdle()
                onNode(end).assert(hasText("18:00:00", substring = true))
                onNodeWithText("Not saved yet", substring = true).assertExists()
                onNodeWithContentDescription("Save availability").performClick()
                waitUntil(timeoutMillis = 10_000) { puts.size == 2 }
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writes("c6:Availability:$member").state.value.saved }
                waitForIdle()
                onNode(end).assert(hasText("18:00:00", substring = true))
                onNodeWithText("Not saved yet", substring = true).assertDoesNotExist()
            }
            assertEquals("17:30:00", puts[0].second["windows"]!!.jsonArray.single().jsonObject["end"]!!.jsonPrimitive.content)
            assertEquals(1L, puts[0].first)
            assertEquals("18:00:00", puts[1].second["windows"]!!.jsonArray.single().jsonObject["end"]!!.jsonPrimitive.content)
            assertEquals(2L, puts[1].first, "the follow-up save is based on the version the first save produced")
            services.screenSession.writeScope.cancel()
        } finally { cache.close() }
    }
}
