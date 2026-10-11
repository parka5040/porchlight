package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.JsonObject
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2aSettingsTest {
    @Test fun settingsSaveOutlivesPanelAndUsesOneRequest() {
        for (today in listOf(true, false)) withSettings { cache, data, session, member ->
            val reply = CompletableDeferred<Unit>()
            var calls = 0
            var cancelled = false
            suspend fun write() {
                calls++
                try { reply.await() } catch (e: CancellationException) { cancelled = true; throw e }
            }
            val writer = HouseholdSettingsController({ "fixture-key" }, { _, body, _ ->
                write(); WriteResult.Success(cache.view.value.settings!!.copy(settings = body))
            }, { _, _, body, _ -> write(); WriteResult.Success(member.copy(color = body.color!!)) })
            val services = object : AppServices by FakeServices() {
                override val data = data
                override val screenSession = session
                override val settingsWrites = writer
            }
            runDesktopComposeUiTest(width = 320, height = 1400) {
                var visible by mutableStateOf(true)
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) {
                    PorchlightTheme(ThemeMode.Daylight, false) {
                        Column(Modifier.verticalScroll(rememberScrollState())) {
                            if (visible) HouseholdSettingsPanel(services, today) else Text("Other screen")
                        }
                    }
                } }
                onNodeWithContentDescription(if (today) "Save Today settings" else "Green").performScrollTo().performClick()
                waitUntil { writer.state.value.status == SettingsWriteStatus.Saving }
                runOnIdle { visible = false }
                onNodeWithText("Other screen").assertExists()
                runOnIdle {
                    assertFalse(cancelled, "Settings write was cancelled with its panel")
                    reply.complete(Unit)
                }
                waitUntil { writer.state.value.status == SettingsWriteStatus.Saved }
                runOnIdle { visible = true }
                onNodeWithText("Settings saved.").performScrollTo().assertExists()
                assertEquals(1, calls)
                evidence(if (today) "settings-save-320-double" else "color-save-320-double")
            }
        }
    }

    @Test fun serverRangeErrorIsAttachedToTheDaysControl() = withSettings { cache, data, session, _ ->
        val writer = HouseholdSettingsController({ "fixture-key" }, { _, _, _ ->
            throw ApiProblem.decode("""{"type":"validation-failed","status":422,"title":"Invalid","errors":[{"field":"this_week_days","message":"Choose a smaller range"}]}""")
        }, { _, _, _, _ -> error("Not used") })
        runBlocking { writer.saveToday(cache.view.value.settings!!, cache.view.value.settings!!.settings) }
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = session
            override val settingsWrites = writer
        }
        runDesktopComposeUiTest(width = 320, height = 1400) {
            setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) {
                PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { HouseholdSettingsPanel(services, true) }
                }
            } }
            onNodeWithContentDescription("This week days").performScrollTo().assert(hasText("Choose a smaller range"))
            evidence("settings-error-320-double")
            onNodeWithContentDescription("Edit settings").performScrollTo().performClick()
            onNodeWithContentDescription("This week days").performScrollTo().assertIsEnabled()
        }
    }

    private fun withSettings(block: (ReadCache, HouseholdData, ScreenSession, MemberRead) -> Unit) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession()
        val member = MemberRead(false, "self", "household", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        try {
            runBlocking { cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id), members = listOf(member))) }
            val api = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = throw java.io.IOException("Offline")
            }
            block(cache, HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now }), session, member)
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
