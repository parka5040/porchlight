package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.graphics.toArgb
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
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
class AuditAuthzListsTest {
    private val recording get() = checkNotNull(javaClass.getResource("/item-fixtures/list-orphan-redacted.json")).readText()

    @Test fun cleanupDescriptorDecodesWithoutContentFields() {
        val row = WireJson.decodeFromString<ListResponse>(recording)
        assertNull(row.name)
        assertEquals(listOf("delete"), row.capabilities)
        assertTrue(row.propertyEntries.isEmpty())
    }

    @Test fun listsScreenOffersPrivateCleanupAtNarrowDoubleFont() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val descriptor = WireJson.decodeFromString<ListResponse>(recording)
        cache.putList(listFixture("one").copy(id = descriptor.id, itemId = descriptor.itemId, name = "Previously private content"))
        var deletes = 0
        var orphanCollectionFetched = false
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            when {
                req.method == HttpMethod.Delete -> {
                    deletes++
                    assertEquals("1", req.headers["If-Match"])
                    respond("", HttpStatusCode.NoContent)
                }
                req.url.encodedPath == "/v1/lists" -> {
                    assertEquals("true", req.url.parameters["include_orphaned"])
                    orphanCollectionFetched = true
                    respond("{\"items\":[${if (deletes == 0) recording else ""}],\"next_cursor\":null}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                }
                else -> error("Cleanup must not fetch a deleted parent or normal list detail")
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
                    PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen() }
                } }
                waitUntil(timeoutMillis = 10_000) { orphanCollectionFetched && onAllNodesWithContentDescription("Private list").fetchSemanticsNodes().size == 1 }
                assertEquals(listOf(descriptor), cache.view.value.lists, "Cleanup must evict every old content scope")
                onNodeWithText("Previously private content").assertDoesNotExist()
                onNodeWithContentDescription("Private list").performScrollTo().performClick()
                onNodeWithContentDescription("Delete list").performScrollTo().assertIsDisplayed()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val evidence = File("build/test-evidence/audit-authz").apply { mkdirs() }
                assertTrue(ImageIO.write(image, "png", evidence.resolve("private-list-320dp-2x.png")))
                onNodeWithContentDescription("Delete list").performClick()
                onNodeWithContentDescription("Confirm delete list").performClick()
                waitUntil(timeoutMillis = 10_000) { deletes == 1 }
                onNodeWithContentDescription("Open attached item").assertDoesNotExist()
                onNodeWithContentDescription("Rename list").assertDoesNotExist()
            }
        } finally {
            services.screenSession.writeScope.cancel()
            api.close(); cache.close()
        }
    }
}
