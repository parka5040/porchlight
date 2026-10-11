package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.screens.collaboration.CollaborationSection
import app.porchlight.shared.screens.planning.PlanningScreen
import app.porchlight.shared.screens.planning.unplacedReason
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiRetainedTest {
    private val now = Instant.parse("2026-10-11T12:00:00Z")
    private val row = itemRead("item").copy(title = "Private appointment", capabilities = listOf("edit", "view", "comment"))
    private val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
    private fun services(cache: ReadCache, request: suspend (String, String) -> JsonObject): AppServices {
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = request(method, path)
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now }).also { it.planning.afterAccept = {} }
        return object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession(); override val clock = AppClock { now } }
    }
    private fun page() = buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
    @Test fun commentBodyClearsOnEviction() = comment(false)
    @Test fun commentBodyClearsOnRedaction() = comment(true)
    private fun comment(redact: Boolean) = runBlocking {
        val cache = memoryCache { now }; val services = services(cache) { _, _ -> page() }
        val comment = commentFixture().copy(itemId = row.id, authorId = member.id, createdAt = now, body = "Private comment body")
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row), comments = listOf(comment), members = listOf(member), memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(row) {} }
                } } }
                onNodeWithContentDescription("Edit comment").performScrollTo().performClick()
                onAllNodesWithText("Private comment body").assertCountEquals(2)
                runBlocking { if (redact) cache.inSession { cache.putComments(it, listOf(comment.copy(body = "", deletedAt = now))) } else cache.evictComment(comment.id) }
                waitForIdle()
                onNodeWithText("Private comment body").assertDoesNotExist()
                onNodeWithContentDescription("Save comment").assertDoesNotExist()
                auditEvidence("comment-cleared-$redact")
            }
        } finally { services.screenSession!!.writeScope.cancel(); cache.close() }
    }
    @Test fun conflictComparisonDisappearsWithEvictedItem() = runBlocking {
        val cache = memoryCache { now }
        val server = itemResponse(row.id).copy(title = row.title)
        val current = WireJson.encodeToJsonElement(server.copy(notes = "Private current notes", version = 2)).jsonObject
        val services = services(cache) { method, _ -> if (method == "PATCH") throw ApiProblem("version-conflict", 409, "Changed", extras = buildJsonObject { put("current", current) }) else WireJson.encodeToJsonElement(server).jsonObject }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen(row.id) } } }
                onNodeWithContentDescription("Edit ${row.title}").performScrollTo().performClick()
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Private draft notes")
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { onAllNodesWithText("Current: Private current notes").fetchSemanticsNodes().isNotEmpty() }
                runBlocking { cache.evictItem(row.id) }; waitForIdle()
                onNodeWithText("Current: Private current notes").assertDoesNotExist()
                onNodeWithText("Yours: Private draft notes").assertDoesNotExist()
                assertNull(services.screenSession!!.writes("item:${row.id}").state.value.current, "retained write current must be cleared")
                auditEvidence("conflict-cleared")
            }
        } finally { services.screenSession!!.writeScope.cancel(); cache.close() }
    }
    @Test fun displayedUnplacedDisappearsOnEviction() = unplaced(false)
    @Test fun delayedUnplacedCannotCrossEviction() = unplaced(true)
    private fun unplaced(delayed: Boolean) = runBlocking {
        val cache = memoryCache { now }; val release = CompletableDeferred<Unit>(); val entered = CompletableDeferred<Unit>()
        val response = SuggestResponse("run", emptyList(), listOf(Unplaced("occ", UnplacedReason.no_slot_before_deadline)))
        val services = services(cache) { method, _ -> if (method == "POST") { entered.complete(Unit); if (delayed) release.await(); WireJson.encodeToJsonElement(response).jsonObject } else page() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row), occurrences = listOf(occurrenceRead("occ", row.id)), members = listOf(member), memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) { PlanningScreen(services, SettingsSection.Planning) } } }
                waitUntil { runCatching { onNodeWithContentDescription("Request suggestions").assertIsEnabled() }.isSuccess }
                onNodeWithContentDescription("Request suggestions").performScrollTo().performClick()
                waitUntil { entered.isCompleted }
                val reason = unplacedReason(UnplacedReason.no_slot_before_deadline)
                if (!delayed) waitUntil { onAllNodesWithText(reason, substring = true).fetchSemanticsNodes().isNotEmpty() }
                runBlocking { cache.evictItem(row.id) }; release.complete(Unit)
                waitUntil { services.screenSession!!.writes("c6:Planning:self").state.value.saved }
                onNodeWithText(reason, substring = true).assertDoesNotExist()
                auditEvidence("unplaced-cleared-$delayed")
            }
        } finally { release.complete(Unit); services.screenSession!!.writeScope.cancel(); cache.close() }
    }
    @Test fun boardDragDoesNotSubmitAnEvictedCard() = runDesktopComposeUiTest(width = 1000, height = 1600) {
        val card = row.copy(capabilities = listOf("change_status"), occurrence = occurrenceRead("occ", row.id))
        var rows by mutableStateOf(listOf(card)); var moves = 0
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { BoardContent(boardColumns(rows, null, "self"), listOf(member), {}, { _, _ -> moves++ }) } }
        onNodeWithTag("board-card-occ").performTouchInput { down(Offset(20f, 20f)); advanceEventTime(900); moveBy(Offset(0f, 30f)) }
        runOnIdle { rows = emptyList() }; waitForIdle()
        onRoot().performTouchInput { up() }
        assertEquals(0, moves)
        onNodeWithText(row.title).assertDoesNotExist()
        auditEvidence("board-drag-evicted")
    }
}
