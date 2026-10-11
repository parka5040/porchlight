package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.*
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientDataFix1ListSearchTest {
    @Test fun displayedSearchHitDisappearsOnSameVersionRedaction() = interleaving(false)
    @Test fun delayedSearchReplyCannotCrossSameVersionRedaction() = interleaving(true)
    private fun interleaving(delayed: Boolean) = runBlocking {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val readable = listFixture().copy(itemId = collabId("item"), name = "Private fixture name")
        val redacted = readable.copy(redacted = true, name = null, propertyEntries = emptyList(), capabilities = listOf("delete"))
        var collection = readable
        val searches = AtomicInteger()
        val release = CompletableDeferred<Unit>()
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            val row = if (req.url.encodedPath == "/v1/search") {
                searches.incrementAndGet()
                if (delayed) release.await()
                JsonObject(WireJson.encodeToJsonElement(readable).jsonObject + ("entity_type" to JsonPrimitive("list")))
            } else WireJson.encodeToJsonElement(collection)
            respond(buildJsonObject { put("items", JsonArray(listOf(row))); put("next_cursor", JsonNull) }.toString(),
                headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = ScreenSession()
        }
        try {
            runDesktopComposeUiTest(width = 400, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) {
                    PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen() }
                } }
                waitUntil(timeoutMillis = 10_000) { cache.view.value.lists.isNotEmpty() }
                onNodeWithText("Search lists").performTextInput("fixture")
                onNodeWithContentDescription("Search lists").performClick()
                waitUntil(timeoutMillis = 10_000) { searches.get() == 1 }
                if (!delayed) waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Open Private fixture name").fetchSemanticsNodes().isNotEmpty() }
                collection = redacted
                runBlocking { data.lists.refresh(includeOrphaned = true) }
                release.complete(Unit)
                waitForIdle()
                waitUntil(timeoutMillis = 10_000) { services.screenSession.writeScope.coroutineContext[Job]!!.children.none { it.isActive } }
                waitForIdle()
                onNodeWithContentDescription("Open Private fixture name").assertDoesNotExist()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val evidence = java.io.File("build/test-evidence/audit-client-data-fix1").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", evidence.resolve("list-search-redacted-$delayed.png")))
                runBlocking { cache.evictList(readable.id) }
                waitForIdle()
                onNodeWithContentDescription("Open Private fixture name").assertDoesNotExist()
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); api.close(); cache.close() }
    }
}
