package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.LocalScreenServices
import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientSyncFixEStorageNoticeUiTest {
    @Test fun compactDoubleFontShowsActionableCleanupNoticeAndRecovery() = notice(320)
    @Test fun expandedDoubleFontShowsActionableCleanupNoticeAndRecovery() = notice(1280)

    private fun notice(width: Int) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun remove(key: String) { error("Storage unavailable") }
        }
        val cache = memoryCache(store) { now }
        val screenSession = ScreenSession()
        val wire = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when {
                path.startsWith("/v1/snapshot") -> WireJson.encodeToJsonElement(emptySnapshot(2)).jsonObject
                path.startsWith("/v1/board") || path.startsWith("/v1/agenda") -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> error("No other reads required")
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ wire }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = screenSession
            override val runtimeNotice = cache.storageNotice
        }
        try {
            runBlocking {
                val row = attachmentFixture()
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
                assertFailsWith<IllegalStateException> { cache.evictAttachment(row.id) }
            }
            runDesktopComposeUiTest(width = width, height = 1400) {
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, 2f), LocalScreenServices provides services) {
                        PorchlightTheme(ThemeMode.Daylight, false) {
                            ShellNavigation(AppRoute.Today, DeviceSettings(), services.settings, {}, services = services)
                        }
                    }
                }
                onNodeWithText(ATTACHMENT_CLEANUP_NOTICE).assertIsDisplayed()
                onNodeWithContentDescription("Clear local cache and refresh from server").assertIsDisplayed()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val directory = java.io.File("build/test-evidence/client-sync-fix-e").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("storage-$width-double.png")))
                onNodeWithContentDescription("Clear local cache and refresh from server").performClick()
                onNodeWithText("Confirm clear local cache").performClick()
                waitUntil(timeoutMillis = 5_000) { cache.sync.value?.cursor == 2L }
                onNodeWithText(ATTACHMENT_CLEANUP_NOTICE).assertDoesNotExist()
                onNodeWithContentDescription("Clear local cache and refresh from server").assertDoesNotExist()
                assertTrue(cache.view.value.attachments.isEmpty())
            }
        } finally { screenSession.close(); data.close() }
    }
}
