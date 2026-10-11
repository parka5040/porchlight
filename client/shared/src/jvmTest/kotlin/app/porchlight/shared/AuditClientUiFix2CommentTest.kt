package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.CollaborationSection
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiFix2CommentTest {
    @Test fun cancelEditReleasesOldCommentDependency() = newDraft(false)
    @Test fun successfulEditReleasesOldCommentDependency() = newDraft(true)
    private fun newDraft(save: Boolean) = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }
        val row = itemRead("item").copy(capabilities = listOf("view", "edit", "comment"))
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        var comment = commentFixture().copy(itemId = row.id, authorId = member.id, createdAt = now, body = "Prior comment")
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") return buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
                assertEquals("PATCH", method)
                assertEquals(comment.version.toLong(), version)
                comment = comment.copy(body = body!!.getValue("body").jsonPrimitive.content, version = comment.version + 1)
                return WireJson.encodeToJsonElement(comment).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }).also { it.comments.afterWrite = {} }
        val services = object : AppServices by FakeServices() {
            override val data = data; override val screenSession = ScreenSession(); override val clock = AppClock { now }
        }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(row), comments = listOf(comment), members = listOf(member),
                memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(row) {} }
                } } }
                onNodeWithContentDescription("Edit comment").performScrollTo().performClick()
                onNode(hasText("Edit comment") and hasSetTextAction()).performScrollTo().performTextReplacement("Edited comment")
                if (save) {
                    onNodeWithContentDescription("Save comment").performScrollTo().performClick()
                    waitUntil { services.screenSession.writes("collaboration:item").state.value.saved }
                    assertEquals("Edited comment", comment.body)
                } else onNodeWithContentDescription("Cancel comment edit").performScrollTo().performClick()
                onNodeWithText("Add a comment").performScrollTo().performTextReplacement("Independent new draft")
                runBlocking { cache.evictComment(comment.id) }
                waitForIdle()
                onNodeWithText("Add a comment").assertTextContains("Independent new draft")
                onNodeWithContentDescription("Post comment").assertIsEnabled()
            }
        } finally { services.screenSession.writeScope.cancel(); cache.close() }
    }
}
