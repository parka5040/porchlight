package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.screens.today.defaultTodaySettings
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiItemTest {
    @Test fun newerTypingSurvivesSave() = saving(false, false)
    @Test fun newerTypingSurvivesOffScreenSave() = saving(true, false)
    @Test fun newerTypingSurvivesCreation() = saving(false, true)
    private fun saving(away: Boolean, creating: Boolean) = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }
        val release = CompletableDeferred<Unit>()
        val patches = CopyOnWriteArrayList<JsonObject>(); val versions = CopyOnWriteArrayList<Long?>()
        var server = itemResponse("item").copy(title = "Laundry", version = 1)
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "PATCH" || method == "POST") {
                    patches += checkNotNull(body); versions += version
                    if (patches.size == 1) release.await()
                    server = server.copy(title = body["title"]?.jsonPrimitive?.content ?: server.title,
                        notes = body["notes"]?.jsonPrimitive?.content ?: server.notes, version = server.version + 1)
                }
                return WireJson.encodeToJsonElement(server).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }).also { it.items.afterWrite = {} }
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(itemRead("item").copy(title = "Laundry")), memberSettings = SettingsRead("self", 1, defaultTodaySettings))) }
            runDesktopComposeUiTest(width = 320, height = 1600) {
                var visible by mutableStateOf(true)
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (visible) ItemScreen(if (creating) "new" else "item", creating = creating)
                } } }
                if (!creating) { waitUntil { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }; onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick() }
                onNodeWithContentDescription("Title").performTextReplacement("Saved title")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { patches.size == 1 }
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Newer unsaved notes")
                if (away) { runOnIdle { visible = false }; waitForIdle() }
                release.complete(Unit)
                waitUntil { services.screenSession.writes("item:${if (creating) "new" else "item"}").state.value.saved }
                if (away) runOnIdle { visible = true }
                waitForIdle()
                onNodeWithContentDescription("Notes").assertExists("newer input must remain in the editor")
                onNodeWithContentDescription("Notes").assertTextContains("Newer unsaved notes")
                onNodeWithText("Unsaved changes").assertExists()
                auditEvidence("item-save-$away-$creating")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { patches.size == 2 }
                assertEquals(JsonPrimitive("Newer unsaved notes"), patches[1]["notes"])
                assertEquals(2L, versions[1], "next save uses committed version, including after create")
                assertFalse(patches[1].containsKey("title"), "already-saved fields must not be resent")
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); cache.close() }
    }
    @Test fun evictedItemDraftCannotReturnAfterFreshRead() = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z"); val cache = memoryCache { now }
        val server = itemResponse("item").copy(title = "Laundry")
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = WireJson.encodeToJsonElement(server).jsonObject
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(itemRead("item").copy(title = "Laundry")))) }
            runDesktopComposeUiTest(width = 320, height = 1600) {
                var visible by mutableStateOf(true)
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { if (visible) ItemScreen("item") } } }
                waitUntil { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick()
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Evicted private draft")
                runOnIdle { visible = false }; waitForIdle()
                runBlocking { cache.evictItem("item") }
                runOnIdle { visible = true }
                waitUntil { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick()
                onNodeWithText("Evicted private draft").assertDoesNotExist()
                auditEvidence("item-evicted-draft")
            }
        } finally { services.screenSession.writeScope.cancel(); cache.close() }
    }
}
