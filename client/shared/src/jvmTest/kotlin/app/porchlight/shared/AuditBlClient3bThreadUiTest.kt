package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.CollaborationSection
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.runTest
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.seconds

@OptIn(ExperimentalTestApi::class)
class AuditBlClient3bThreadUiTest {
    @Test fun completeHistoryAndWindowSurviveCompositionRecreation() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val pages = AtomicInteger()
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            if (req.url.encodedPath.endsWith("comments")) pages.incrementAndGet()
            respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val clock = AppClock { now }
            override val screenSession = ScreenSession()
        }
        try {
            val item = itemRead(collabId("item")).copy(capabilities = listOf("view"))
            cache.install(emptySnapshot(1).copy(items = listOf(item), comments = List(70) {
                commentFixture().copy(id = "comment-$it", body = "History $it", createdAt = now + it.seconds)
            }))
            runDesktopComposeUiTest(width = 320, height = 900) {
                var generation by mutableIntStateOf(0)
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    key(generation) { Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(item) {} } }
                } } }
                onNodeWithContentDescription("Show older").performScrollTo().performClick()
                waitUntil { onAllNodesWithText("History 0").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Show older").assertIsNotEnabled()
                runOnIdle { generation++ }
                onNodeWithText("History 0").assertExists()
                onNodeWithContentDescription("Show older").assertIsNotEnabled()
                assertEquals(0, pages.get(), "Authoritative snapshot already has all history")
                onNodeWithContentDescription("Show older").performScrollTo()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val directory = java.io.File("build/test-evidence/audit-bl-client3b").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("thread-320-font2.png")))
            }
        } finally { services.screenSession.writeScope.cancel(); api.close(); cache.close() }
    }
}
