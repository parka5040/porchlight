package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.Source
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.LocalScreenServices
import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.screens.planning.FeedContent
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.ui.MemberChip
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2aPresentationTest {
    @Test fun paletteInitialsUseMemberInkToken() = runDesktopComposeUiTest(width = 320, height = 640) {
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { MemberChip("Sam Rivera", 0) } }
        val layouts = mutableListOf<TextLayoutResult>()
        onNodeWithText("SR", useUnmergedTree = true).fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action!!.invoke(layouts)
        assertEquals(PorchlightTokens.Daylight.memberInk, layouts.single().layoutInput.style.color)
    }
    @Test fun feedCredentialsAreHiddenUntilExplicitReveal() = runDesktopComposeUiTest(width = 320, height = 1400) {
        val url = "https://example.org/private-calendar?access=fixture-value"
        val feed = WireJson.decodeFromString<Source>("""{"id":"source","household_id":"h","owner_member_id":"self","kind":"feed","visibility":"private","import_key":"feed:test","name":"School","url":"$url","allow_http":false,"refresh_seconds":3600,"enabled":true,"next_fetch_at":"2026-10-06T10:00:00Z","last_warnings":[],"failure_count":0,"version":1}""")
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) { FeedContent(rows = listOf(feed)) } } }
        onNodeWithText(url).assertDoesNotExist()
        onNodeWithContentDescription("Reveal URL for School").performScrollTo().performClick()
        onNodeWithText(url).assertExists()
        onNodeWithContentDescription("Copy URL for School").assertExists()
        onNodeWithContentDescription("Hide URL for School").performScrollTo().performClick()
        onNodeWithText(url).assertDoesNotExist()
        evidence("feed-320-double")
    }
    @Test fun missingCachedItemDoesNotClaimDeletion() {
        val now = Instant.parse("2026-10-07T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession()
        try {
            runBlocking { cache.install(emptySnapshot(1)) }
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = throw java.io.IOException("Offline")
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
            runDesktopComposeUiTest(width = 320, height = 1400) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f), LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    ShellNavigation(AppRoute.Item("missing"), DeviceSettings(), services.settings, {}, services = services)
                } } }
                onNodeWithText("This item is not in saved data. Reconnect and open it again.").assertExists()
                onNodeWithText("This item is no longer available.").assertDoesNotExist()
                evidence("offline-notice-320-double")
            }
        } finally { session.writeScope.cancel(); cache.close() }
    }
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = java.io.File("build/test-evidence/audit-bl-client2a").apply { mkdirs() }
        assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("$name.png")))
    }
}
