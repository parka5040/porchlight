package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.files.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient3aExportUiTest {
    @Test fun householdPickerPrecedesStepUpAndDownloadShowsByteProgress() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession()
        val release = CompletableDeferred<Unit>()
        var picked = 0; var authorized = 0; var downloaded = 0; var finished = false
        var selected: (suspend (ExportSink) -> Unit)? = null
        var complete: ((Result<Boolean>) -> Unit)? = null
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
            override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
                assertEquals(1, authorized); assertEquals("fixture-step", stepUp)
                downloaded++; consume(ByteArray(8192)); release.await()
            }
        }
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "self"),
                members = listOf(MemberRead(false, "self", "h", "Sam Rivera", "super_admin", "#FABD2F", "SR", "UTC", "manual", 1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() {
                override val data = data
                override val screenSession = session
                override suspend fun stepUp(password: String, action: String): String {
                    assertEquals(2, picked); assertNotNull(selected); assertEquals("household.export", action)
                    authorized++; return "fixture-step"
                }
            }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) = error("Not used")
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) {
                    assertTrue(household); picked++; selected = write; complete = onResult
                }
            }
            runDesktopComposeUiTest(width = 320, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services,
                    androidx.compose.ui.platform.LocalDensity provides androidx.compose.ui.unit.Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    PlanningScreen(services, SettingsSection.Export, files)
                } } }
                onNodeWithContentDescription("Export household").performScrollTo().performClick()
                runOnIdle { assertEquals(1, picked, "Pick destination before requesting password"); assertEquals(0, authorized); complete!!(Result.success(false)) }
                onNodeWithText("Password").assertDoesNotExist()
                onNodeWithContentDescription("Export household").performScrollTo().performClick()
                runOnIdle { session.writeScope.launch {
                    val sink = object : ExportSink {
                        override suspend fun write(bytes: ByteArray) {}
                        override suspend fun finish(checkGate: () -> Unit) { checkGate(); finished = true }
                        override suspend fun abort() {}
                    }
                    try { selected!!(sink); complete!!(Result.success(true)) }
                    catch (e: Exception) { complete!!(Result.failure(e)) }
                } }
                waitUntil { onAllNodes(hasText("Password") and hasSetTextAction()).fetchSemanticsNodes().size == 1 }
                onNode(hasText("Password") and hasSetTextAction()).performTextInput("fixture password")
                onNodeWithContentDescription("Confirm household export").performClick()
                waitUntil { downloaded == 1 }
                onNodeWithText("Downloaded 8192 bytes · validating before saving…").performScrollTo().assertIsDisplayed()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val directory = java.io.File("build/test-evidence/audit-bl-client3a").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("export-progress-320-double.png")))
                runOnIdle { release.complete(Unit) }
                waitUntil { onAllNodesWithText("Export saved").fetchSemanticsNodes().size == 1 }
                assertTrue(finished)
                onNodeWithText("Export saved").assertExists()
            }
        } finally { release.complete(Unit); session.writeScope.cancel(); cache.close() }
    }
}
