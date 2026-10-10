package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class C3aConnectedScreensTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun services(data: HouseholdData): AppServices = object : AppServices by FakeServices() {
        override val data = data
        override val clock = AppClock { now }
        override val screenSession = ScreenSession()
    }
    @Test fun todayCompletionUsesOccurrenceVersionAndServerReadStateWithoutOptimism() = runTest {
        val cache = memoryCache { now }
        val requests = mutableListOf<Pair<String?, String?>>()
        val reply = kotlinx.coroutines.CompletableDeferred<Unit>()
        val api = transport { req ->
            requests += req.headers["Idempotency-Key"] to req.headers["If-Match"]
            reply.await()
            assertEquals("/v1/occurrences/occ/complete", req.url.encodedPath)
            respond(WireJson.encodeToString(OccurrenceResponse.serializer(), occurrenceResponse("occ", "a").copy(status = OccurrenceStatus.done)),
                headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.occurrences.afterWrite = {}
            val row = itemRead("a").copy(title = "Laundry", capabilities = listOf("view", "complete"))
            cache.install(emptySnapshot(1).copy(items = listOf(row), occurrences = listOf(occurrenceRead("occ", "a").copy(dueDate = kotlinx.datetime.LocalDate.parse("2026-10-06"))),
                memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.today), SettingsScope.mine, 7))))
            val services = services(data)
            val visible = mutableStateOf(true)
            runDesktopComposeUiTest(width = 600, height = 1000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { if (visible.value) TodayScreen() else androidx.compose.material3.Text("Away") } } }
                onNodeWithContentDescription("Complete Laundry").performClick()
                waitUntil(timeoutMillis = 10_000) { requests.size == 1 }
                onNodeWithContentDescription("Complete Laundry").assertExists() // no authoritative optimistic removal
                runOnIdle { visible.value = false }
                onNodeWithText("Away").assertExists()
                runOnIdle { reply.complete(Unit) }
                waitUntil(timeoutMillis = 10_000) { services.screenSession!!.writes("today").state.value.saved }
                runOnIdle { visible.value = true }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Saved").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Complete Laundry").assertDoesNotExist()
                onNodeWithText("Nothing here.").assertExists()
            }
            assertEquals(1, requests.size); assertNotNull(requests.single().first); assertEquals("1", requests.single().second)
            assertEquals("done", cache.view.value.occurrences.single().status)
        } finally { api.close(); cache.close() }
    }
    @Test fun repeatedNewItemVisitsDoNotReplayPriorSuccessOrCreateDuplicates() = runTest {
        val cache = memoryCache { now }
        var created = 0
        val api = transport { req ->
            assertEquals(HttpMethod.Post, req.method); assertEquals("/v1/items", req.url.encodedPath)
            created++
            respond(WireJson.encodeToString(ItemResponse.serializer(), itemResponse("item-$created")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }); data.items.afterWrite = {}
            cache.install(emptySnapshot(1).copy(memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
            val services = services(data)
            val creating = mutableStateOf(true); var visit = 0
            runDesktopComposeUiTest(width = 650, height = 1200) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (creating.value) key(visit) { ItemScreen("new", creating = true, navigate = { creating.value = false }) }
                    else app.porchlight.shared.ui.ShellButton("Create another", onClick = { visit++; creating.value = true })
                } } }
                repeat(2) { index ->
                    onNodeWithContentDescription("Title").performTextReplacement("Task $index")
                    onNodeWithContentDescription("Save item").performClick()
                    waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Create another").fetchSemanticsNodes().isNotEmpty() }
                    if (index == 0) onNodeWithContentDescription("Create another").performClick()
                }
            }
            assertEquals(2, created)
        } finally { api.close(); cache.close() }
    }
    @Test fun editor409ShowsYoursCurrentAndExplicitReapplyUsesNewVersionAndKey() = runTest {
        val cache = memoryCache { now }
        val versions = mutableListOf<String?>()
        val keys = mutableListOf<String?>()
        val edits = mutableListOf<JsonObject>()
        var saved = false
        val api = transport { req ->
            if (req.method == HttpMethod.Patch) {
                versions += req.headers["If-Match"]; keys += req.headers["Idempotency-Key"]
                edits += WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                if (versions.size == 1) respond(buildJsonObject {
                    put("type", "urn:porchlight:problem:version-conflict"); put("status", 409); put("title", "Changed")
                    put("current", WireJson.encodeToJsonElement(itemResponse("a").copy(title = "Server title", notes = "Concurrent notes", version = 2)))
                }.toString(), HttpStatusCode.Conflict, headersOf(HttpHeaders.ContentType, "application/problem+json"))
                else { saved = true; respond(WireJson.encodeToString(ItemResponse.serializer(), itemResponse("a").copy(title = "My title", version = 3)), headers = headersOf(HttpHeaders.ContentType, "application/json")) }
            } else respond(WireJson.encodeToString(ItemResponse.serializer(), itemResponse("a").copy(title = if (saved) "My title" else "Laundry", version = if (saved) 3 else 1)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.items.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry")), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
            val services = services(data)
            runDesktopComposeUiTest(width = 650, height = 1200) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Evening, false) { ItemScreen("a") } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performClick()
                onNodeWithContentDescription("Title").performTextReplacement("My title")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Current", substring = true).fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Yours", substring = true).assertExists()
                onNodeWithText("Server title", substring = true).assertExists()
                onNodeWithText("Concurrent notes", substring = true).assertDoesNotExist()
                assertEquals(1, versions.size) // no automatic version overwrite/retry
                onNodeWithContentDescription("Reapply after comparison").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Saved").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit My title").assertExists()
            }
            assertEquals(listOf<String?>("1", "2"), versions)
            assertNotEquals(keys[0], keys[1]); assertEquals(setOf("title"), edits[0].keys); assertEquals(setOf("title"), edits[1].keys)
            assertEquals(JsonPrimitive("My title"), edits[1]["title"])
        } finally { api.close(); cache.close() }
    }

    @Test fun recurringScopePreservesTypedTitleAndSendsNoHeadTiming() = runTest {
        val cache = memoryCache { now }
        val patches = mutableListOf<JsonObject>()
        val response = itemResponse("a").copy(title = "Laundry", dueDate = kotlinx.datetime.LocalDate.parse("2026-10-01"), recurrenceMode = ItemRecurrenceMode.calendar, rrule = "FREQ=DAILY;COUNT=10")
        val api = transport { req ->
            if (req.method == HttpMethod.Patch) {
                val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                patches += body
                assertEquals("this_and_future", req.url.parameters["scope"]); assertEquals("occ", req.url.parameters["occurrence_id"])
                respond(WireJson.encodeToString(ItemResponse.serializer(), response.copy(title = "Typed title", version = 2)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            } else respond(WireJson.encodeToString(ItemResponse.serializer(), response), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }); data.items.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry", dueDate = response.dueDate, recurrenceMode = "calendar", rrule = response.rrule)),
                occurrences = listOf(occurrenceRead("occ", "a").copy(seq = 4, dueDate = kotlinx.datetime.LocalDate.parse("2026-10-05"))), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
            val services = services(data)
            runDesktopComposeUiTest(width = 650, height = 1200) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Evening, false) { ItemScreen("a", "occ") } } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performClick()
                onNodeWithContentDescription("Title").performScrollTo().performTextReplacement("Typed title")
                // These edits are against the head, not the seq-4 tail.
                onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-02")
                onNodeWithContentDescription("Count").performScrollTo().performTextReplacement("9")
                onNodeWithContentDescription("Save item").performClick()
                onNodeWithText("Choose a recurring edit scope before saving.").assertExists()
                assertTrue(patches.isEmpty())
                onNodeWithContentDescription("Edit scope: This and future").performScrollTo().performClick()
                onNodeWithContentDescription("Title").assertTextContains("Typed title")
                onNodeWithContentDescription("Due date").performScrollTo().assertTextContains("2026-10-05")
                onNodeWithContentDescription("Count").performScrollTo().assertTextContains("6")
                // Any subsequent timing edit is now made against the tail seed.
                onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-06")
                onNodeWithContentDescription("Count").performScrollTo().performTextReplacement("5")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil(timeoutMillis = 10_000) { patches.isNotEmpty() }
            }
            assertEquals(buildJsonObject { put("title", "Typed title"); put("due_date", "2026-10-06"); put("rrule", "FREQ=DAILY;COUNT=5") }, patches.single())
        } finally { api.close(); cache.close() }
    }

    @Test fun restoredTailDraftChoosingAllUsesHeadTimingAndExactBody() = runTest {
        for (editHeadTiming in listOf(false, true)) {
            val cache = memoryCache { now }
            val patches = mutableListOf<JsonObject>()
            val response = itemResponse("a").copy(title = "Laundry", dueDate = kotlinx.datetime.LocalDate.parse("2026-10-01"), recurrenceMode = ItemRecurrenceMode.calendar, rrule = "FREQ=DAILY;COUNT=10")
            val api = transport { req ->
                if (req.method == HttpMethod.Patch) {
                    patches += WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                    assertEquals("/v1/items/a", req.url.encodedPath)
                    assertEquals("all", req.url.parameters["scope"])
                    assertEquals(null, req.url.parameters["occurrence_id"])
                    respond(WireJson.encodeToString(ItemResponse.serializer(), response.copy(title = "Typed title", version = 2)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                } else respond(WireJson.encodeToString(ItemResponse.serializer(), response), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }); data.items.afterWrite = {}
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry", dueDate = response.dueDate, recurrenceMode = "calendar", rrule = response.rrule)),
                    occurrences = listOf(occurrenceRead("occ", "a").copy(seq = 4, dueDate = kotlinx.datetime.LocalDate.parse("2026-10-05"))), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
                val services = services(data)
                val visible = mutableStateOf(true)
                var restoredDate = ""
                var restoredCount = ""
                runDesktopComposeUiTest(width = 650, height = 1200) {
                    setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Evening, false) {
                        if (visible.value) ItemScreen("a", "occ") else androidx.compose.material3.Text("Away")
                    } } }
                    waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                    onNodeWithContentDescription("Edit Laundry").performClick()
                    onNodeWithContentDescription("Title").performScrollTo().performTextReplacement("Typed title")
                    onNodeWithContentDescription("Edit scope: This and future").performScrollTo().performClick()
                    onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-06")
                    onNodeWithContentDescription("Count").performScrollTo().performTextReplacement("5")
                    // Back-navigation disposes the screen, not the application-owned draft/baseline.
                    runOnIdle { visible.value = false }
                    onNodeWithText("Away").assertExists()
                    runOnIdle {
                        assertEquals("2026-10-06", services.screenSession!!.drafts["a"]!!.dueDate)
                        assertEquals("2026-10-05", services.screenSession!!.drafts["a:baseline"]!!.dueDate)
                        visible.value = true
                    }
                    waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                    onNodeWithContentDescription("Edit Laundry").performClick()
                    onNodeWithContentDescription("Edit scope: All occurrences").performScrollTo().performClick()
                    runOnIdle {
                        restoredDate = services.screenSession!!.drafts["a"]!!.dueDate
                        restoredCount = services.screenSession!!.drafts["a"]!!.count
                    }
                    if (editHeadTiming) {
                        onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-02")
                        onNodeWithContentDescription("Count").performScrollTo().performTextReplacement("9")
                    }
                    onNodeWithContentDescription("Save item").performClick()
                    waitUntil(timeoutMillis = 10_000) { patches.isNotEmpty() }
                }
                assertEquals(buildJsonObject {
                    put("title", "Typed title")
                    if (editHeadTiming) { put("due_date", "2026-10-02"); put("rrule", "FREQ=DAILY;COUNT=9") }
                }, patches.single(), "restored All, editHeadTiming=$editHeadTiming")
                assertEquals("2026-10-01", restoredDate)
                assertEquals("10", restoredCount)
            } finally { api.close(); cache.close() }
        }
    }

    @Test fun defaultClearsSurviveAllAndFutureScopeAndReachTransport() = runTest {
        for (chosen in listOf("All occurrences", "This and future")) {
            val cache = memoryCache { now }
            val patches = mutableListOf<JsonObject>()
            val response = itemResponse("a").copy(title = "Laundry", notes = "Old notes", address = "Old address", priority = 3,
                recurrenceMode = ItemRecurrenceMode.calendar, rrule = "FREQ=DAILY;COUNT=10")
            val api = transport { req ->
                if (req.method == HttpMethod.Patch) {
                    patches += WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                    assertEquals(if (chosen == "This and future") "this_and_future" else "all", req.url.parameters["scope"])
                    assertEquals(if (chosen == "This and future") "occ" else null, req.url.parameters["occurrence_id"])
                }
                respond(WireJson.encodeToString(ItemResponse.serializer(), response), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }); data.items.afterWrite = {}
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a").copy(title = "Laundry", notes = response.notes.orEmpty(), address = response.address,
                    priority = 3, recurrenceMode = "calendar", rrule = response.rrule)),
                    occurrences = listOf(occurrenceRead("occ", "a").copy(seq = 4)), memberSettings = SettingsRead("self", 1, defaultTodaySettings)))
                val services = services(data)
                runDesktopComposeUiTest(width = 650, height = 1200) {
                    setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("a", "occ") } } }
                    waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                    onNodeWithContentDescription("Edit Laundry").performClick()
                    onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("")
                    onNodeWithContentDescription("Address").performScrollTo().performTextReplacement("")
                    onNodeWithContentDescription("Priority").performScrollTo().performTextReplacement("0")
                    // Scroll the vertical form via its label; the choice has a nearer horizontal scroller.
                    onNodeWithText("Recurrence").performScrollTo()
                    onNodeWithContentDescription("Recurrence: none").performClick()
                    runOnIdle { assertEquals("none", services.screenSession!!.drafts["a"]!!.recurrence, "before $chosen") }
                    onNodeWithContentDescription("Edit scope: $chosen").performScrollTo().performClick()
                    runOnIdle {
                        val draft = services.screenSession!!.drafts["a"]!!
                        assertEquals("none", draft.recurrence, chosen)
                        assertEquals(emptyMap(), draft.errors(), chosen)
                        assertEquals("", draft.notes, chosen); assertEquals("", draft.address, chosen); assertEquals("0", draft.priority, chosen)
                    }
                    onNodeWithContentDescription("Save item").performClick()
                    waitUntil(timeoutMillis = 10_000) { patches.isNotEmpty() }
                }
                assertEquals(buildJsonObject { put("notes", ""); put("address", JsonNull); put("priority", 0); put("recurrence_mode", JsonNull); put("rrule", JsonNull) }, patches.single(), chosen)
            } finally { api.close(); cache.close() }
        }
    }
}
