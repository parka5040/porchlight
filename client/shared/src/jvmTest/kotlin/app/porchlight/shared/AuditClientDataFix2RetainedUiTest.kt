package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.screens.planning.PlanningScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientDataFix2RetainedUiTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun editorClosesOnRedaction() = editor(false)
    @Test fun editorClosesOnEviction() = editor(true)
    private fun editor(evict: Boolean) = runBlocking {
        val cache = memoryCache { now }
        val entry = entryFixture("Private label", "a").copy(notes = "Private notes", url = "https://example.invalid/private", qty = 42.0)
        val readable = listFixture().copy(propertyEntries = listOf(entry))
        val redacted = readable.copy(redacted = true, name = null, propertyEntries = emptyList(), capabilities = listOf("delete"))
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = WireJson.encodeToJsonElement(readable).jsonObject
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.putList(it, readable) }
            runDesktopComposeUiTest(width = 400, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen(readable.id) } } }
                onNodeWithContentDescription("Edit Private label").performScrollTo().performClick()
                onAllNodesWithText("Private notes").assertCountEquals(2)
                runBlocking { if (evict) cache.evictList(readable.id) else cache.inSession { cache.putList(it, redacted) } }
                waitForIdle()
                onNodeWithText("Edit entry").assertDoesNotExist()
                onNodeWithText("Private notes").assertDoesNotExist()
                onNodeWithContentDescription("Save entry").assertDoesNotExist()
                evidence("editor-$evict")
            }
        } finally { services.screenSession.writeScope.cancel(); cache.close() }
    }
    @Test fun delayedCandidatesCannotCrossItemEviction() = candidates(true, false)
    @Test fun displayedCandidatesDisappearOnItemEviction() = candidates(false, false)
    @Test fun delayedCandidatesCannotCrossOccurrenceEviction() = candidates(true, true)
    @Test fun displayedCandidatesDisappearOnOccurrenceEviction() = candidates(false, true)
    private fun candidates(delayed: Boolean, occurrence: Boolean) = runBlocking {
        val cache = memoryCache { now }
        val item = itemRead("appointment").copy(kind = "appointment", assignees = emptyList(), subjectMemberId = "dependent")
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val result = CandidatesResponse(item.id, listOf(Candidate(member.id, CandidateReason.entries.first())), false, "occurrence")
        val release = CompletableDeferred<Unit>(); val calls = AtomicInteger()
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path.contains("candidates")) { calls.incrementAndGet(); if (delayed) release.await(); return WireJson.encodeToJsonElement(result).jsonObject }
                return buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(item), occurrences = listOf(occurrenceRead("occurrence", item.id)),
                members = listOf(member, member.copy(id = "dependent", role = "dependent", displayName = "Dependent fixture")),
                memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id))) }
            runDesktopComposeUiTest(width = 400, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { PlanningScreen(services, SettingsSection.Planning) } } }
                waitUntil(timeoutMillis = 10_000) { runCatching { onNodeWithContentDescription("Dependent assignee candidates").assertIsEnabled() }.isSuccess }
                waitForIdle()
                onNodeWithContentDescription("Dependent assignee candidates").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { calls.get() == 1 }
                val card = "Sam Rivera · ${app.porchlight.shared.screens.planning.candidateReason(result.candidates.single().reason)}"
                if (!delayed) waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(card).fetchSemanticsNodes().isNotEmpty() }
                runBlocking { if (occurrence) cache.evictOccurrence("occurrence") else cache.evictItem(item.id) }
                release.complete(Unit)
                waitUntil(timeoutMillis = 10_000) { runCatching { onNodeWithContentDescription("Dependent assignee candidates").assertIsEnabled() }.isSuccess }
                waitForIdle()
                onNodeWithText(card).assertDoesNotExist()
                evidence("candidates-$delayed-$occurrence")
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); cache.close() }
    }
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = java.io.File("build/test-evidence/audit-client-data-fix2").apply { mkdirs() }
        assertTrue(javax.imageio.ImageIO.write(image, "png", directory.resolve("$name.png")))
    }
}
