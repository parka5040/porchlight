package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientSyncFixFEditorTest {
    @Test fun existingUnsavedEditorReappearsAfterFreshAuthorisation() {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession().also { it.bind(0); it.bindCache(cache) }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) =
                WireJson.encodeToJsonElement(itemResponse("item")).jsonObject
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("item")))
            runBlocking { cache.install(snapshot) }
            session.editor("item", cache.view.value, false).current(cache.view.value)!!.apply {
                baseline = EditorDraft.from(itemRead("item")); expectedVersion = 1; editing = true
                store(baseline.copy(title = "Keep unsaved edit"))
            }
            runDesktopComposeUiTest(width = 1280, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) {
                    PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("item") }
                } }
                onNodeWithContentDescription("Title").assertTextContains("Keep unsaved edit")
                runBlocking { cache.clearLocalCache() }
                onNodeWithContentDescription("Title").assertDoesNotExist()
                runBlocking { cache.install(snapshot) }
                onNodeWithContentDescription("Title").assertTextContains("Keep unsaved edit")
                onNodeWithText("Save item").assertExists()
            }
        } finally { session.close(); data.close() }
    }

    @Test fun realItemScreenRetriesAmbiguousRelationshipStepWhileEditorIsQuarantined() {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession().also { it.bind(0); it.bindCache(cache) }
        val sent = mutableListOf<Pair<String, String?>>()
        val response = itemResponse("item").copy(title = "Local edit", version = 2)
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                sent += method to idempotencyKey
                if (method == "PUT" && sent.count { it.first == "PUT" } == 1) throw ApiProblem("invalid-response", 502, "Reply unavailable")
                return WireJson.encodeToJsonElement(response).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        data.items.afterWrite = {}
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
        try {
            runBlocking { cache.install(emptySnapshot(1).copy(items = listOf(itemRead("item")))) }
            val retained = session.editor("new", cache.view.value, true)
            retained.current(cache.view.value)!!.apply {
                savedId = "item"; expectedVersion = 1
                baseline = EditorDraft.from(itemRead("item"))
                store(baseline.copy(title = "Local edit", assignees = listOf("fixture-member")))
            }
            retained.bindResources(cache.view.value, setOf(CacheResource("Item", "item")))
            runDesktopComposeUiTest(width = 1280, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) {
                    PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("new", creating = true) }
                } }
                onNodeWithText("Save item").performClick()
                waitUntil(timeoutMillis = 5_000) { session.writes("item:new").state.value.retryKey != null }
                runBlocking { cache.clearLocalCache() }
                onNodeWithText("Your edits are kept but hidden", substring = true).assertExists()
                onNodeWithContentDescription("Title").assertDoesNotExist()
                onNodeWithText("Retry identical request").performClick()
                waitUntil(timeoutMillis = 5_000) { session.writes("item:new").state.value.saved }
                val steps = sent.filter { it.first != "GET" }
                assertEquals(listOf("PATCH", "PUT", "PUT"), steps.map { it.first })
                assertEquals(steps[1].second, steps[2].second, "The ambiguous relationship step must keep its original key")
                assertNotEquals(steps[0].second, steps[1].second)
            }
        } finally { session.close(); data.close() }
    }
}
