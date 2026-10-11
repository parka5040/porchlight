package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.screens.collaboration.CollaborationSection
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
class AuditClientUiFix1DraftTest {
    private val now = Instant.parse("2026-10-11T12:00:00Z")
    private val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
    private val caps = listOf("view", "edit", "comment", "reassign", "change_visibility", "edit_reminders")
    private data class Request(val path: String, val body: JsonObject, val version: Long?)
    @Test fun unsavedCreationSurvivesRemoval() = editor(creating = true, submit = false)
    @Test fun rejectedCreationSurvivesRemoval() = editor(creating = true, rejection = 422)
    @Test fun conflictingCreationSurvivesRemoval() = editor(creating = true, rejection = 409)
    @Test fun unrelatedEvictionPreservesInFlightItemDraft() = editor(eviction = true)
    @Test fun unrelatedEvictionPreservesNewDraft() = editor(creating = true, submit = false, eviction = true)
    @Test fun allCreationFieldsSurviveAndSaveAfterCreate() = editor(creating = true, relationships = true)
    @Test fun relationshipOnlyChangesCanBeSaved() = editor(creating = true, relationships = true, notes = false)
    @Test fun partialRelationshipRejectionKeepsRemainingDraft() = editor(creating = true, relationships = true, partialFailure = true)
    private fun editor(creating: Boolean = false, submit: Boolean = true, rejection: Int? = null, eviction: Boolean = false, relationships: Boolean = false, notes: Boolean = true, partialFailure: Boolean = false) = runBlocking {
        val cache = memoryCache { now }; val release = CompletableDeferred<Unit>()
        val requests = CopyOnWriteArrayList<Request>()
        var failViewers = partialFailure
        var server = itemResponse("item").copy(title = "Laundry", capabilities = caps.map(ItemCapability::valueOf))
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method != "GET") {
                    requests += Request(path, checkNotNull(body), version)
                    if (requests.size == 1) {
                        release.await()
                        if (rejection != null) throw ApiProblem(if (rejection == 409) "version-conflict" else "validation-failed", rejection, "Rejected", extras = buildJsonObject { put("current", WireJson.encodeToJsonElement(server)) })
                    }
                    if (path.endsWith("/viewers") && failViewers) { failViewers = false; throw ApiProblem("validation-failed", 422, "Rejected viewers") }
                    if (version != null) assertEquals(server.version.toLong(), version, "each endpoint uses preceding committed version")
                    server = server.copy(title = body["title"]?.jsonPrimitive?.content ?: server.title,
                        notes = body["notes"]?.jsonPrimitive?.content ?: server.notes,
                        assignees = body["assignees"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.assignees,
                        viewers = body["viewers"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.viewers,
                        reminderOffsets = body["reminder_offsets"]?.jsonArray?.map { it.jsonPrimitive.int } ?: server.reminderOffsets,
                        version = server.version + 1)
                }
                return WireJson.encodeToJsonElement(server).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }).also { it.items.afterWrite = {} }
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(itemRead("item").copy(title = "Laundry", capabilities = caps), itemRead("other")), members = listOf(member), memberSettings = SettingsRead("self", 1, defaultTodaySettings))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                var visible by mutableStateOf(true)
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (visible) ItemScreen(if (creating) "new" else "item", creating = creating)
                } } }
                if (!creating) { waitUntil { onAllNodesWithContentDescription("Edit Laundry").fetchSemanticsNodes().isNotEmpty() }; onNodeWithContentDescription("Edit Laundry").performScrollTo().performClick() }
                onNodeWithContentDescription("Title").performTextReplacement("Submitted title")
                if (submit) { onNodeWithContentDescription("Save item").performClick(); waitUntil { requests.isNotEmpty() } }
                if (notes) onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Newer notes")
                if (relationships) {
                    onNodeWithContentDescription("Assignees: Sam Rivera").performScrollTo().performClick()
                    onNodeWithContentDescription("Viewers: Sam Rivera").performScrollTo().performClick()
                    onNodeWithContentDescription("Reminder offsets (minutes)").performScrollTo().performTextReplacement("15,30")
                }
                runOnIdle { visible = false }; waitForIdle()
                if (eviction) runBlocking { cache.evictItem("other"); cache.evictList("other-list"); cache.evictComment("other-comment") }
                release.complete(Unit)
                val writer = services.screenSession.writes("item:${if (creating) "new" else "item"}")
                waitUntil { !writer.state.value.busy }
                runOnIdle { visible = true }; waitForIdle()
                onNodeWithContentDescription("Notes").assertExists("unsaved editor must be reachable after re-entry")
                if (notes) onNodeWithContentDescription("Notes").assertTextContains("Newer notes")
                if (relationships) {
                    onNodeWithContentDescription("Assignees: Sam Rivera").assertExists("newer assignees must remain actionable")
                    onNodeWithContentDescription("Viewers: Sam Rivera").assertExists()
                    onNodeWithContentDescription("Reminder offsets (minutes)").assertTextContains("15,30")
                    onNodeWithContentDescription("Save item").performClick()
                    waitUntil { requests.size > 1 && !writer.state.value.busy }
                    if (partialFailure) {
                        assertNotNull(writer.state.value.error)
                        assertEquals("Newer notes", server.notes, "earlier endpoint already committed")
                        onNodeWithContentDescription("Reminder offsets (minutes)").assertTextContains("15,30")
                        val before = requests.size
                        onNodeWithContentDescription("Save item").performClick()
                        waitUntil { requests.size > before && !writer.state.value.busy }
                        assertEquals(1, requests.count { it.path.endsWith("/assignees") }, "do not replay committed steps")
                    }
                    assertEquals(listOf("self"), server.assignees, "second save must commit newer assignees")
                    assertEquals(listOf("self"), server.viewers, "second save must commit newer viewers")
                    assertEquals(listOf(15, 30), server.reminderOffsets, "second save must commit newer reminders")
                    if (notes) assertEquals("Newer notes", server.notes)
                    assertEquals((if (notes) 5 else 4) + (if (partialFailure) 1 else 0), requests.size, "POST, changed fields only, and three versioned relationship writes")
                    assertFalse(services.screenSession.drafts.containsKey("new"), "retire only fully saved draft")
                }
                auditEvidence("fix1-draft-$creating-$submit-$rejection-$eviction-$relationships")
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); cache.close() }
    }
    @Test fun unrelatedEvictionPreservesCommentDraft() = runBlocking {
        val cache = memoryCache { now }; val row = itemRead("item").copy(capabilities = caps)
        val data = HouseholdData(cache, GeneratedCall({ error("No requests needed") }, { null }), { now })
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row, itemRead("other")))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(row) {} }
                } } }
                onNodeWithText("Add a comment").performScrollTo().performTextReplacement("Authorized draft")
                runBlocking { cache.evictItem("other"); cache.evictComment("other-comment") }
                waitForIdle()
                onNodeWithText("Add a comment").assertTextContains("Authorized draft")
            }
        } finally { services.screenSession.writeScope.cancel(); cache.close() }
    }
}
