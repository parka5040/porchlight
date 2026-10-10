package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class CacheVisibilityUiRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun services(data: HouseholdData): AppServices = object : AppServices by FakeServices() {
        override val data = data
        override val clock = AppClock { now }
        override val screenSession = ScreenSession()
    }
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun MockRequestHandleScope.json(body: String) = respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
    private fun MockRequestHandleScope.notFound() = respond("""{"type":"urn:porchlight:problem:not-found","status":404,"title":"Unavailable"}""", HttpStatusCode.NotFound, headersOf(HttpHeaders.ContentType, "application/problem+json"))

    @Test fun c503OldSearchHitCannotRestoreEvictedSelectedList() = runTest {
        val cache = memoryCache { now }; val list = listFixture(); var revoked = false; var gets = 0
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/search" -> json(buildJsonObject { put("items", JsonArray(listOf(JsonObject(WireJson.encodeToJsonElement(list).jsonObject + ("entity_type" to JsonPrimitive("list")))))); put("next_cursor", JsonNull) }.toString())
                "/v1/lists" -> json(buildJsonObject { put("items", JsonArray(if (revoked) emptyList() else listOf(WireJson.encodeToJsonElement(list)))); put("next_cursor", JsonNull) }.toString())
                else -> { gets++; if (revoked) notFound() else json(WireJson.encodeToString(ListResponse.serializer(), list)) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); val services = services(data)
        try {
            cache.putList(list)
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen(list.id) } } }
                waitUntil(timeoutMillis = 10_000) { gets > 0 }
                onNodeWithText("Search lists").performTextInput("Groceries")
                onNodeWithContentDescription("Search lists").performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Open Groceries").fetchSemanticsNodes().isNotEmpty() }
                revoked = true
                onNodeWithContentDescription("Refresh lists").performClick()
                waitUntil(timeoutMillis = 10_000) { cache.view.value.lists.isEmpty() }
                waitForIdle()
                val stale = onAllNodesWithContentDescription("Open Groceries").fetchSemanticsNodes()
                if (stale.isNotEmpty()) { onNodeWithContentDescription("Open Groceries").performClick(); waitForIdle() }
                assertTrue(cache.view.value.lists.isEmpty(), "Search result restored revoked content")
                onNodeWithContentDescription("Open Groceries").assertDoesNotExist()
                onNodeWithText(list.propertyEntries.first().label, substring = false).assertDoesNotExist()
            }
        } finally { services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }

    @Test fun c503StaleNotYetEvictedSearchHitGetsBeforeSelection() = runTest {
        val cache = memoryCache { now }; val list = listFixture(); var gets = 0
        val release = kotlinx.coroutines.CompletableDeferred<Unit>()
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/search" -> json(buildJsonObject { put("items", JsonArray(listOf(JsonObject(WireJson.encodeToJsonElement(list).jsonObject + ("entity_type" to JsonPrimitive("list")))))); put("next_cursor", JsonNull) }.toString())
                "/v1/lists" -> json(buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(list)))); put("next_cursor", JsonNull) }.toString())
                else -> { gets++; release.await(); notFound() }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); val services = services(data)
        try {
            cache.putList(list)
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen() } } }
                onNodeWithText("Search lists").performTextInput("Groceries")
                onNodeWithContentDescription("Search lists").performClick()
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Open Groceries").fetchSemanticsNodes().isNotEmpty() }
                assertTrue(cache.view.value.lists.isNotEmpty())
                onNodeWithContentDescription("Open Groceries").performClick()
                waitUntil(timeoutMillis = 10_000) { gets == 1 }
                onNodeWithText(list.propertyEntries.first().label, substring = false).assertDoesNotExist()
                assertTrue(cache.view.value.lists.isNotEmpty(), "stale row must still be cached while GET is pending")
                release.complete(Unit)
                waitUntil(timeoutMillis = 10_000) { cache.view.value.lists.isEmpty() }
                waitForIdle()
                onNodeWithText(list.propertyEntries.first().label, substring = false).assertDoesNotExist()
                onNodeWithContentDescription("Open Groceries").assertDoesNotExist()
            }
        } finally { release.complete(Unit); services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }
    @Test fun unavailableParentStillShowsAmbiguousWriteRetry() = runTest {
        val cache = memoryCache { now }
        val api = transport { notFound() }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); val services = services(data)
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            services.screenSession!!.bind(cache.sessionEpoch())
            services.screenSession!!.writes("item:one").submit { throw java.io.IOException("ambiguous write") }
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("one") } } }
                waitUntil(timeoutMillis = 10_000) { cache.view.value.items.isEmpty() }
                waitForIdle()
                onNodeWithContentDescription("Retry identical request").assertExists()
                onNodeWithContentDescription("Edit Title").assertDoesNotExist()
                onNodeWithContentDescription("Save item").assertDoesNotExist()
            }
        } finally { services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }
    @Test fun d07RefetchLocksAnAlreadyOpenEditorAfterVisibilityLoss() = runTest {
        val cache = memoryCache { now }; var revoked = false; var gets = 0
        val api = transport { req ->
            if (req.url.encodedPath == "/v1/items/one") { gets++; if (revoked) notFound() else json(WireJson.encodeToString(ItemResponse.serializer(), itemResponse("one"))) }
            else json("""{"items":[],"next_cursor":null}""")
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); val services = services(data)
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("one") } } }
                waitUntil(timeoutMillis = 10_000) { gets > 0 }
                onNodeWithContentDescription("Edit Title").performClick()
                onNodeWithContentDescription("Save item").assertExists()
                revoked = true
                onNodeWithContentDescription("Refetch item").performClick()
                waitUntil(timeoutMillis = 10_000) { gets >= 2 }
                waitForIdle()
                onNodeWithContentDescription("Save item").assertDoesNotExist()
                onNodeWithText("Title", substring = false).assertDoesNotExist()
            }
        } finally { services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }
}
