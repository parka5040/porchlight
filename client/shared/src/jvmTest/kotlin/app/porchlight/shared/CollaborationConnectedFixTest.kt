package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.*
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.cancel
import app.porchlight.shared.screens.today.defaultTodaySettings
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class CollaborationConnectedFixTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private inline fun <reified T> recording(name: String): T = WireJson.decodeFromString(checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() })
    private fun services(data: HouseholdData): AppServices = object : AppServices by FakeServices() {
        override val data = data
        override val clock = AppClock { now }
        override val screenSession = ScreenSession()
    }
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })

    @Test fun attachedPersistentRouteAndRealCapabilitiesPermitRenameNotVisibility() = runTest {
        val list = recording<ListResponse>("list-attached-persistent")
        val cache = memoryCache { now }
        var fetched = false
        val api = transport { req ->
            assertEquals(HttpMethod.Get, req.method)
            assertNull(req.url.parameters["occurrence_id"])
            fetched = true
            respond(WireJson.encodeToString(ListResponse.serializer(), list), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            assertEquals(listOf("view", "check", "manage", "edit", "delete"), list.capabilities)
            cache.putList(list)
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
            val services = services(data)
            runDesktopComposeUiTest(width = 700, height = 2400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen(list.id, collabId("occurrence")) } } }
                waitUntil(timeoutMillis = 10_000) { fetched }
                onNodeWithContentDescription("Rename list").performScrollTo().assertExists()
                onNodeWithContentDescription("Visibility household").assertDoesNotExist()
                onNodeWithText("Selected list is not cached for this occurrence.").assertDoesNotExist()
            }
            services.screenSession!!.writeScope.cancel()
        } finally { api.close(); cache.close() }
    }

    @Test fun realAssigneeCapabilitiesAllowUncheckButNotClearOrEntryManagement() = runDesktopComposeUiTest(width = 700, height = 2400) {
        val list = recording<ListResponse>("list-persistent-assignee")
        assertEquals(listOf("view", "check"), list.capabilities)
        assertNotNull(list.gotIt.single().checkedAt)
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { ListContent(list, emptyList(), { _, _ -> }, {}, { _, _ -> }, {}, {}) } }
        onNodeWithContentDescription("Uncheck Oat milk").assertExists()
        onNodeWithContentDescription("Clear Got it").assertDoesNotExist()
        onNodeWithContentDescription("Edit Oat milk").assertDoesNotExist()
    }
    @Test fun successfulListAndEntryCreationClearTheirDrafts() = runTest {
        var list = recording<ListResponse>("list")
        val cache = memoryCache { now }
        var creates = 0; var adds = 0
        val api = transport { req ->
            val reply = when {
                req.method == HttpMethod.Get && req.url.encodedPath == "/v1/lists" -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(list)))); put("next_cursor", JsonNull) }.toString()
                req.method == HttpMethod.Get -> WireJson.encodeToString(ListResponse.serializer(), list)
                req.url.encodedPath == "/v1/lists" -> { creates++; WireJson.encodeToString(ListResponse.serializer(), list) }
                else -> {
                    adds++
                    val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                    val entry = list.propertyEntries.first().copy(label = body["label"]!!.jsonPrimitive.content)
                    list = list.copy(propertyEntries = list.propertyEntries + entry)
                    WireJson.encodeToString(EntryResponse.serializer(), entry)
                }
            }
            respond(reply, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); data.lists.afterWrite = {}
            val services = services(data)
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen() } } }
                onNodeWithText("New list name").performTextInput("Groceries")
                onNodeWithContentDescription("Create list").performClick()
                waitUntil(timeoutMillis = 10_000) { creates == 1 && onAllNodesWithContentDescription("Add entry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Create list").assertIsNotEnabled()
                onNodeWithText("Add to Groceries").performTextInput("Apples")
                onNodeWithContentDescription("Add entry").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { adds == 1 && services.screenSession!!.writes("lists:${list.id}").state.value.saved }
                onNodeWithContentDescription("Add entry").assertIsNotEnabled()
            }
            assertEquals(1, creates); assertEquals(1, adds)
            services.screenSession!!.writeScope.cancel()
        } finally { api.close(); cache.close() }
    }

    @Test fun successfulCommentPostAndEditClearDraftAndPersistentAttachmentNavigationOmitsOccurrence() = runTest {
        val list = recording<ListResponse>("list-attached-persistent")
        val cache = memoryCache { now }
        var comment = recording<Comment>("comment").copy(itemId = list.itemId!!, authorId = list.ownerId)
        var posts = 0; var edits = 0
        val api = transport { req ->
            val reply = when {
                req.method == HttpMethod.Get && req.url.encodedPath == "/v1/lists" -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(list)))); put("next_cursor", JsonNull) }.toString()
                req.method == HttpMethod.Get -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(comment)))); put("next_cursor", JsonNull) }.toString()
                else -> {
                    if (req.method == HttpMethod.Post) posts++ else { edits++; assertEquals(comment.version.toString(), req.headers["If-Match"]) }
                    val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject["body"]!!.jsonPrimitive.content
                    comment = comment.copy(body = body, version = comment.version + 1)
                    WireJson.encodeToString(Comment.serializer(), comment)
                }
            }
            respond(reply, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }); data.comments.afterWrite = {}
            cache.install(emptySnapshot(1).copy(memberSettings = SettingsRead(list.ownerId, 1, defaultTodaySettings), lists = listOf(list)))
            val services = services(data)
            var route: AppRoute? = null
            val item = itemRead(list.itemId!!).copy(capabilities = listOf("view", "edit", "comment"), occurrence = occurrenceRead(collabId("occurrence"), list.itemId!!))
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { androidx.compose.foundation.layout.Column { CollaborationSection(item) { route = it } } } } }
                onNodeWithContentDescription("Open Daily groceries").performClick()
                assertEquals(AppRoute.Lists(list.id, null), route)
                onNodeWithContentDescription("Create per-occurrence list").assertDoesNotExist()
                onNodeWithText("Add a comment").performTextInput("A new comment")
                onNodeWithContentDescription("Post comment").performClick()
                waitUntil(timeoutMillis = 10_000) { posts == 1 && services.screenSession!!.writes("collaboration:${item.id}").state.value.saved }
                onNodeWithContentDescription("Post comment").assertIsNotEnabled()
                onNodeWithContentDescription("Edit comment").performClick()
                onNode(hasText("Edit comment") and hasSetTextAction()).performTextReplacement("Updated comment")
                onNodeWithContentDescription("Save comment").performClick()
                waitUntil(timeoutMillis = 10_000) { edits == 1 && services.screenSession!!.writes("collaboration:${item.id}").state.value.saved }
                onNodeWithContentDescription("Save comment").assertDoesNotExist()
                onNodeWithContentDescription("Post comment").assertIsNotEnabled()
            }
            assertEquals(1, posts); assertEquals(1, edits)
            services.screenSession!!.writeScope.cancel()
        } finally { api.close(); cache.close() }
    }
}
