package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.screens.today.defaultTodaySettings
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

/**
 * fbcc m6c-astra C5-06: a picker/camera result that returns after its launching composition was discarded (rotation,
 * PIN relock at ON_STOP) is kept for the item, never uploaded while locked, and uploaded once to the target chosen at
 * launch after unlock. The platform side (Android Activity -> AttachmentPicks) is covered by AttachmentPickRegistryTest.
 */
@OptIn(ExperimentalTestApi::class)
class AttachmentPickRelockTest {
    private val now = Instant.parse("2026-10-06T12:05:00Z")
    private class Upload(val path: String, val key: String, val bytes: ByteArray)

    private suspend fun fixture(uploads: MutableList<Upload>, comment: Comment): Pair<ReadCache, AppServices> {
        val cache = memoryCache { now }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject =
                buildJsonObject { put("items", buildJsonArray {}); put("next_cursor", JsonNull) }
            override suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, session: String?, key: String, progress: (Long, Long) -> Unit): JsonObject {
                uploads += Upload(path, key, bytes)
                return WireJson.encodeToJsonElement(Attachment(collabId("attachment"), comment.itemId, name, now, 1, bytes.size.toLong(), mime, false,
                    commentId = path.removePrefix("/v1/comments/").substringBefore('/').takeIf { path.startsWith("/v1/comments/") }, uploaderId = collabId("self"))).jsonObject
            }
        }
        cache.install(emptySnapshot(1).copy(memberSettings = SettingsRead(collabId("self"), 1, defaultTodaySettings), comments = listOf(comment)))
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
        data.attachments.afterWrite = {}
        return cache to object : AppServices by FakeServices() {
            override val data = data
            override val clock = AppClock { now }
            override val screenSession = ScreenSession()
        }
    }

    @Test fun pickReturnedWhileRelockedUploadsOnceToTheLaunchTargetAfterUnlock() = runTest {
        val comment = commentFixture()
        val uploads = mutableListOf<Upload>()
        val (cache, services) = fixture(uploads, comment)
        val session = services.screenSession!!
        val item = itemRead(comment.itemId).copy(capabilities = listOf("view", "comment"))
        val photo = "jpeg bytes".encodeToByteArray()
        try {
            val visible = mutableStateOf(true)
            runDesktopComposeUiTest(width = 700, height = 3000) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (visible.value) androidx.compose.foundation.layout.Column { CollaborationSection(item) {} } else Text("Locked")
                } } }
                onNodeWithText("Comments").assertExists()
                // "Attach to comment" launched a camera; the full-screen camera stops MainActivity, the PIN gate relocks.
                lateinit var request: AttachmentPicks.Request
                runOnIdle { request = session.attachmentPicks.begin(item.id, comment.id); visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { assertTrue(session.attachmentPicks.complete(request.id, PickedAttachment("photo.jpg", "image/jpeg", photo), null)) }
                waitForIdle()
                assertTrue(uploads.isEmpty(), "never uploads while locked")
                runOnIdle { visible.value = true }
                waitUntil(timeoutMillis = 10_000) { uploads.size == 1 && session.writes("collaboration:${item.id}").state.value.saved }
                assertEquals("/v1/comments/${comment.id}/attachments", uploads.single().path, "target captured at launch, not reset by the relock")
                assertContentEquals(photo, uploads.single().bytes)
                assertTrue(session.attachmentPicks.waiting(item.id).isEmpty())
                // Another relock/unlock cycle does not upload it again.
                runOnIdle { visible.value = false }
                runOnIdle { visible.value = true }
                waitForIdle()
                assertEquals(1, uploads.size)
            }
            session.writeScope.cancel()
        } finally { cache.close() }
    }

    @Test fun identityChangeDropsAParkedPickWithoutUploading() = runTest {
        val comment = commentFixture()
        val uploads = mutableListOf<Upload>()
        val (cache, services) = fixture(uploads, comment)
        val session = services.screenSession!!
        val item = itemRead(comment.itemId).copy(capabilities = listOf("view", "comment"))
        try {
            val picks = session.attachmentPicks
            val parked = picks.begin(item.id, null)
            assertTrue(picks.complete(parked.id, PickedAttachment("a.txt", "text/plain", byteArrayOf(1)), null))
            val late = picks.begin(item.id, null)
            session.bind(41); session.bind(42) // sign-out / another identity
            assertTrue(picks.waiting(item.id).isEmpty())
            assertFalse(picks.complete(late.id, PickedAttachment("b.txt", "text/plain", byteArrayOf(2)), null), "late result of the old identity is refused")
            assertTrue(picks.waiting(item.id).isEmpty())
            assertTrue(uploads.isEmpty())
        } finally { cache.close() }
    }
}
