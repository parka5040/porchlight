package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.CollaborationSection
import app.porchlight.shared.screens.lists.ListsScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2bCollaborationTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun services(data: HouseholdData) = object : AppServices by FakeServices() {
        override val data = data
        override val screenSession = ScreenSession()
        override val clock = AppClock { now }
    }
    @Test fun coldPersistentRouteMakesOnlyOneAuthoritativeGet() = runTest {
        val gets = AtomicInteger()
        val row = listFixture()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                gets.incrementAndGet()
                assertFalse(path.contains("occurrence_id"))
                return WireJson.encodeToJsonElement(row).jsonObject
            }
        }
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
        val services = services(data)
        try {
            runDesktopComposeUiTest(width = 700, height = 2400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { ListsScreen(row.id, "occurrence") } } }
                waitUntil { cache.view.value.lists.isNotEmpty() }
                waitForIdle()
                assertEquals(1, gets.get(), "Persistent template is already the authoritative reply; mode discovery must not restart the effect")
            }
        } finally { services.screenSession.writeScope.cancel(); data.close() }
    }

    @Test fun typingDuringCommentCreateRebasesOntoCommittedCommentInsteadOfDuplicatingIt() = runTest {
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val posts = AtomicInteger(); val edits = AtomicInteger()
        val comment = commentFixture().copy(createdAt = now)
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") return buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
                if (method == "POST") { posts.incrementAndGet(); started.complete(Unit); release.await() } else { edits.incrementAndGet(); assertEquals(1L, version) }
                return WireJson.encodeToJsonElement(comment.copy(body = body!!.getValue("body").jsonPrimitive.content, version = if (method == "POST") 1 else 2)).jsonObject
            }
        }
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }).also { it.comments.afterWrite = {} }
        val services = services(data)
        try {
            val row = itemRead(comment.itemId).copy(capabilities = listOf("view", "comment"))
            cache.install(emptySnapshot(1).copy(items = listOf(row), memberSettings = emptySnapshot(1).memberSettings.copy(memberId = comment.authorId!!)))
            runDesktopComposeUiTest(width = 700, height = 2400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) { Column { CollaborationSection(row) {} } } } }
                onNodeWithText("Add a comment").performTextInput("First text")
                onNodeWithContentDescription("Post comment").performClick()
                waitUntil { started.isCompleted }
                onNodeWithText("Add a comment").performTextReplacement("First text plus more")
                release.complete(Unit)
                waitUntil { services.screenSession.writes("collaboration:${row.id}").state.value.saved }
                onNodeWithContentDescription("Save comment").assertExists().performClick()
                waitUntil { edits.get() == 1 && services.screenSession.writes("collaboration:${row.id}").state.value.saved }
                assertEquals(1, posts.get())
                onNodeWithContentDescription("Post comment").assertIsNotEnabled()
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); data.close() }
    }
}
