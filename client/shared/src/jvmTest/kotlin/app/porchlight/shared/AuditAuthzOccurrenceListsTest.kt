package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.ListResponse
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditAuthzOccurrenceListsTest {
    private fun row(name: String) = WireJson.decodeFromString<ListResponse>(checkNotNull(javaClass.getResource("/item-fixtures/$name.json")).readText())

    @Test fun occurrenceContextCanDeleteUnscopedCleanupDescriptors() = runTest {
        for (redact in listOf(true, false)) {
            val now = Instant.parse("2026-10-06T12:00:00Z")
            val cache = memoryCache { now }
            val scoped = row("list-occurrence-check")
            val cleanup = if (redact) row("list-orphan-redacted").copy(id = scoped.id, itemId = scoped.itemId, mode = scoped.mode, version = scoped.version)
                else scoped.copy(occurrenceId = null, capabilities = listOf("view", "delete"), propertyEntries = emptyList(), gotIt = emptyList())
            cache.putList(scoped)
            var deletes = 0
            var details = 0
            var refreshed = false
            val api = KtorApiTransport(HttpClient(MockEngine { req ->
                when {
                    req.method == HttpMethod.Delete -> {
                        assertEquals(scoped.version.toString(), req.headers["If-Match"])
                        deletes++
                        respond("", HttpStatusCode.NoContent)
                    }
                    req.url.encodedPath == "/v1/lists" -> {
                        refreshed = true
                        respond("{\"items\":[${if (deletes == 0) WireJson.encodeToString(ListResponse.serializer(), cleanup) else ""}],\"next_cursor\":null}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                    }
                    deletes > 0 -> respond("", HttpStatusCode.NotFound) // post-write snapshot/refetch
                    else -> {
                        assertFalse(refreshed, "cleanup must not request occurrence detail")
                        details++
                        respond(WireJson.encodeToString(ListResponse.serializer(), scoped), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                    }
                }
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
            val services = object : AppServices by FakeServices() {
                override val data = data
                override val clock = AppClock { now }
                override val screenSession = ScreenSession()
            }
            try {
                runDesktopComposeUiTest(width = 320, height = 2200) {
                    setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) {
                        PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen(scoped.id, scoped.occurrenceId) }
                    } }
                    waitUntil(timeoutMillis = 10_000) { details == 1 }
                    onNodeWithContentDescription("Refresh lists").performScrollTo().performClick()
                    waitUntil(timeoutMillis = 10_000) { cache.view.value.lists == listOf(cleanup) }
                    onNodeWithText("Selected list is not cached for this occurrence.").assertDoesNotExist()
                    onNodeWithContentDescription("Delete list").performScrollTo().assertIsDisplayed().performClick()
                    onNodeWithContentDescription("Confirm delete list").performClick()
                    waitUntil(timeoutMillis = 10_000) { deletes == 1 }
                }
            } finally {
                services.screenSession.writeScope.cancel()
                api.close(); cache.close()
            }
        }
    }
}
