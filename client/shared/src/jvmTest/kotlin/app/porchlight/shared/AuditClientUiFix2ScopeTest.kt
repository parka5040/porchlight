package app.porchlight.shared

import androidx.compose.runtime.*
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
import kotlinx.datetime.LocalDate
import kotlinx.serialization.json.*
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiFix2ScopeTest {
    @Test fun occurrenceSaveKeepsCreationRelationshipsAndNotes() = scoped("This occurrence")
    @Test fun futureSaveKeepsCreationRelationshipsOnNewTail() = scoped("This and future")
    private data class Request(val method: String, val path: String, val body: JsonObject, val version: Long?)
    private fun scoped(chosen: String) = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }; val release = CompletableDeferred<Unit>()
        val requests = CopyOnWriteArrayList<Request>()
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val caps = listOf("view", "edit", "reassign", "change_visibility", "edit_reminders")
        var server = itemResponse("item").copy(title = "Laundry", dueDate = LocalDate.parse("2026-10-11"), recurrenceMode = ItemRecurrenceMode.calendar,
            rrule = "FREQ=DAILY", capabilities = caps.map(ItemCapability::valueOf))
        var occurrence = occurrenceResponse("occ", "item").copy(dueDate = LocalDate.parse("2026-10-12"))
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") return WireJson.encodeToJsonElement(server).jsonObject
                val fields = checkNotNull(body)
                requests += Request(method, path, fields, version)
                if (requests.size == 1) release.await()
                if (path.startsWith("/v1/occurrences/")) {
                    occurrence = occurrence.copy(overrideDueDate = LocalDate.parse(fields.getValue("override_due_date").jsonPrimitive.content), version = occurrence.version + 1)
                    return WireJson.encodeToJsonElement(occurrence).jsonObject
                }
                if (version != null) assertEquals(server.version.toLong(), version)
                val split = path.contains("scope=this_and_future")
                server = server.copy(id = if (split) "tail" else server.id,
                    notes = fields["notes"]?.jsonPrimitive?.content ?: server.notes,
                    assignees = fields["assignees"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.assignees,
                    viewers = fields["viewers"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.viewers,
                    reminderOffsets = fields["reminder_offsets"]?.jsonArray?.map { it.jsonPrimitive.int } ?: server.reminderOffsets,
                    version = if (split) 1 else server.version + 1)
                if (split) occurrence = occurrence.copy(id = "tail-occ", itemId = "tail")
                return WireJson.encodeToJsonElement(server).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }).also {
            it.items.afterWrite = { cache.inSession { fence -> cache.putOccurrence(fence, occurrence) } }
            it.occurrences.afterWrite = {}
        }
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(members = listOf(member), memberSettings = SettingsRead("self", 1, defaultTodaySettings))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                var visible by mutableStateOf(true)
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    val currentView by cache.view.collectAsState()
                    if (visible) ItemScreen("new", occurrenceId = currentView.occurrences.lastOrNull()?.id, creating = true)
                } } }
                onNodeWithContentDescription("Title").performTextReplacement("Laundry")
                onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-11")
                onNodeWithContentDescription("Recurrence: calendar").performScrollTo().performClick()
                onNodeWithContentDescription("Save item").performClick(); waitUntil { requests.isNotEmpty() }
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Newer notes")
                onNodeWithContentDescription("Assignees: Sam Rivera").performScrollTo().performClick()
                onNodeWithContentDescription("Viewers: Sam Rivera").performScrollTo().performClick()
                onNodeWithContentDescription("Reminder offsets (minutes)").performScrollTo().performTextReplacement("15,30")
                val writer = services.screenSession.writes("item:new")
                release.complete(Unit); waitUntil { !writer.state.value.busy }
                onNodeWithContentDescription("Edit scope: $chosen").performScrollTo().performClick()
                if (chosen == "This occurrence") onNodeWithContentDescription("Due date").performScrollTo().performTextReplacement("2026-10-13")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { requests.size == 2 && !writer.state.value.busy }
                assertTrue(writer.state.value.saved, "scoped mutation must succeed")
                assertTrue(services.screenSession.drafts.containsKey("new"), "scoped save must not retire omitted relationships")
                runOnIdle { visible = false }; waitForIdle(); runOnIdle { visible = true }; waitForIdle()
                onNodeWithContentDescription("Edit scope: All occurrences").performScrollTo().performClick()
                onNodeWithContentDescription("Assignees: Sam Rivera").assertIsSelected()
                onNodeWithContentDescription("Viewers: Sam Rivera").assertIsSelected()
                onNodeWithContentDescription("Reminder offsets (minutes)").performScrollTo().assertTextContains("15,30")
                onNodeWithContentDescription("Notes").performScrollTo().assertTextContains("Newer notes")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { requests.size > 2 && !writer.state.value.busy }
                assertTrue(writer.state.value.saved)
                assertEquals(listOf("self"), server.assignees); assertEquals(listOf("self"), server.viewers)
                assertEquals(listOf(15, 30), server.reminderOffsets); assertEquals("Newer notes", server.notes)
                assertEquals(1, requests.count { it.method == "POST" }, "continuation never creates again")
                assertEquals(3, requests.count { it.method == "PUT" && it.path.startsWith("/v1/items/${server.id}/") })
                if (chosen == "This occurrence") assertEquals(1L, requests[1].version, "scope seeds the occurrence version")
                else assertEquals(1, requests.count { it.body.containsKey("notes") && it.method == "PATCH" }, "saved notes are not resent")
                assertFalse(services.screenSession.drafts.containsKey("new"))
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); cache.close() }
    }
}
