package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import java.io.File
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientSyncFixGCommentTest {
    @Test fun cleanCommentEditorsAreBoundedWithoutDroppingUnsavedDrafts() = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }
        val session = ScreenSession(Dispatchers.Unconfined).also { it.bind(0); it.bindCache(cache) }
        val comments = (0..65).map { commentFixture().copy(id = "comment-$it", itemId = "item-$it", body = "Prior comment") }
        try {
            cache.install(emptySnapshot(1).copy(items = (0..65).map { itemRead("item-$it") }, comments = comments))
            val view = cache.view.value
            val newDraft = session.commentEditor("item-0", view)
            newDraft.current(view)!!.text = "Keep new draft"
            session.beginCommentEdit("item-0", comments[0], view)
            val editDraft = session.commentEditor("item-0", view)
            editDraft.current(view)!!.text = "Keep edited draft"
            session.beginCommentEdit("item-1", comments[1], view)
            val clean = session.commentEditor("item-1", view)
            for (i in 2..65) { session.beginCommentEdit("item-$i", comments[i], view); session.commentEditor("item-$i", view) }
            assertNotSame(clean, session.commentEditor("item-1", view))
            assertSame(editDraft, session.commentEditor("item-0", view))
            session.finishCommentEdit("item-0")
            assertSame(newDraft, session.commentEditor("item-0", view))
            assertEquals("Keep new draft", newDraft.current(view)!!.text)
            session.bind(1)
            assertEquals("", session.commentEditor("item-0", view).current(view)!!.text)
        } finally { session.close(); cache.close() }
    }

    @Test fun newCommentSurvivesQuarantineAndAuthorisedRead() = recovery(false)
    @Test fun existingCommentEditSurvivesQuarantineAndAuthorisedRead() = recovery(true)
    private fun recovery(editing: Boolean) {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }
        val row = itemRead("item").copy(capabilities = listOf("view", "edit", "comment"))
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val comment = commentFixture().copy(itemId = row.id, authorId = member.id, createdAt = now)
        val snapshot = emptySnapshot(1).copy(items = listOf(row), comments = listOf(comment), members = listOf(member),
            memberSettings = emptySnapshot(1).memberSettings.copy(memberId = member.id))
        val hold = CompletableDeferred<Unit>(); val readStarted = CompletableDeferred<Unit>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path.substringBefore('?') == "/v1/items/item") {
                    readStarted.complete(Unit); hold.await()
                    return WireJson.encodeToJsonElement(itemResponse("item")).jsonObject
                }
                return buildJsonObject { put("items", JsonArray(emptyList())) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "original" }), { now })
        val session = ScreenSession().also { it.bind(0); it.bindCache(cache) }
        val services = object : AppServices by FakeServices() {
            override val data = data; override val screenSession = session; override val clock = AppClock { now }
        }
        try {
            runBlocking { cache.install(snapshot) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services, LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
                    androidx.compose.material3.Surface(color = androidx.compose.material3.MaterialTheme.colorScheme.background) { ItemScreen("item") }
                } } }
                waitUntil { readStarted.isCompleted }
                if (editing) {
                    onNodeWithText("Add a comment").performScrollTo().performTextReplacement("Independent unsent comment")
                    onNodeWithContentDescription("Edit comment").performScrollTo().performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.OnClick) { it() }
                    runOnIdle { assertEquals(comment.id, session.commentEditor(row.id, cache.view.value).current(cache.view.value)?.edit?.id) }
                }
                val label = if (editing) "Edit comment" else "Add a comment"
                onNode(hasText(label) and hasSetTextAction()).performScrollTo().performTextReplacement("Keep comment draft")
                runBlocking { cache.clearLocalCache() }
                onNodeWithText("Your edits are kept but hidden", substring = true).assertExists()
                onNode(hasText(label) and hasSetTextAction()).assertDoesNotExist()
                runBlocking { cache.install(snapshot) }
                onNode(hasText(label) and hasSetTextAction()).performScrollTo().assertTextContains("Keep comment draft")
                onNodeWithContentDescription(if (editing) "Save comment" else "Post comment").assertIsEnabled()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val evidence = File("build/test-evidence/client-sync-fix-g").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", evidence.resolve("comment-${if (editing) "edit" else "new"}-320-double.png")))
                if (editing) {
                    onNodeWithContentDescription("Cancel comment edit").performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.OnClick) { it() }
                    onNodeWithText("Add a comment").assertTextContains("Independent unsent comment")
                    onNodeWithContentDescription("Edit comment").performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.OnClick) { it() }
                    onNode(hasText("Edit comment") and hasSetTextAction()).assertTextContains(comment.body)
                }
            }
        } finally { hold.complete(Unit); session.close(); data.close() }
    }
}
