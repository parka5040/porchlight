package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.datetime.LocalDate
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2aEditorTest {
    @Test fun recurringTimingRequiresExplicitScopeEvenWithoutOccurrence() {
        for (withOccurrence in listOf(true, false)) editor(withOccurrence) { services, _ ->
            runDesktopComposeUiTest(width = 320, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) {
                    PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("item", if (withOccurrence) "occ" else null) }
                } }
                onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick()
                onNodeWithContentDescription("Title").performScrollTo().performTextReplacement("Keep title")
                onNodeWithContentDescription("Due date").performScrollTo().assertIsNotEnabled()
                onNodeWithContentDescription("Count").performScrollTo().assertIsNotEnabled()
                onNodeWithContentDescription("Edit scope: All occurrences").performScrollTo().assertIsNotSelected().performClick()
                onNodeWithContentDescription("Due date").performScrollTo().assertIsEnabled()
                onNodeWithContentDescription("Title").performScrollTo().assertTextContains("Keep title")
                evidence(if (withOccurrence) "editor-scope-320-double" else "editor-no-anchor-320-double")
            }
        }
    }

    @Test fun changedAnchorCannotSilentlyReuseThePreviousScope() = editor(true) { services, writes ->
        runDesktopComposeUiTest(width = 320, height = 1400) {
            var anchor by mutableStateOf("occ")
            setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) {
                PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("item", anchor) }
            } }
            onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick()
            onNodeWithContentDescription("Edit scope: This and future").performScrollTo().performClick()
            onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-09")
            runOnIdle { anchor = "other" }
            onNodeWithContentDescription("Due date").assertIsNotEnabled()
            onNodeWithContentDescription("Save item").performClick()
            onNodeWithText("Choose a recurring edit scope before saving.").assertExists()
            assertTrue(writes.isEmpty(), "Changed anchor sent an edit using the previous occurrence")
            onNodeWithContentDescription("Edit scope: This and future").performScrollTo().performClick()
            onNodeWithContentDescription("Due date").performScrollTo().assertIsEnabled().assertTextContains("2026-10-08")
        }
    }

    private fun editor(withOccurrence: Boolean, block: (AppServices, List<JsonObject>) -> Unit) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession().also { it.bindCache(cache) }
        val writes = mutableListOf<JsonObject>()
        val response = itemResponse("item").copy(title = "Laundry", dueDate = LocalDate.parse("2026-10-01"), recurrenceMode = ItemRecurrenceMode.calendar, rrule = "FREQ=DAILY;COUNT=10")
        try {
            runBlocking { cache.install(emptySnapshot(1).copy(items = listOf(itemRead("item").copy(title = "Laundry", dueDate = response.dueDate, recurrenceMode = "calendar", rrule = response.rrule)),
                occurrences = if (withOccurrence) listOf(occurrenceRead("occ", "item").copy(seq = 4, dueDate = LocalDate.parse("2026-10-05")), occurrenceRead("other", "item").copy(seq = 7, dueDate = LocalDate.parse("2026-10-08"))) else emptyList())) }
            val api = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    if (method != "GET") writes += body!!
                    return WireJson.encodeToJsonElement(response).jsonObject
                }
            }
            val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
            data.items.afterWrite = {}
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
            block(services, writes)
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
