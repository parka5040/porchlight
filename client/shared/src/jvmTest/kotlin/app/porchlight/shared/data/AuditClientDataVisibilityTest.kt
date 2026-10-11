package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class AuditClientDataVisibilityTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun childGenerationsSurviveNewerAuthorizedReadsAndDoNotBlockOtherResources() = runTest {
        val cache = memoryCache { now }
        try {
            for (kind in listOf("list", "comment", "attachment")) {
                suspend fun put() { when (kind) {
                    "list" -> cache.putList(listFixture())
                    "comment" -> cache.putComments(listOf(commentFixture()))
                    else -> cache.putAttachment(attachmentFixture())
                } }
                put()
                val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                val old = async {
                    assertFailsWith<StaleVisibilityReply> {
                        cache.inSession { started.complete(Unit); release.await(); put() }
                    }
                }
                started.await()
                when (kind) {
                    "list" -> cache.evictList(listFixture().id)
                    "comment" -> cache.evictComment(commentFixture().id)
                    else -> cache.evictAttachment(attachmentFixture().id)
                }
                cache.inSession { put() } // new authorized read clears the marker, not its generation
                release.complete(Unit); old.await()
            }
            cache.inSession { cache.evictItem("other"); cache.putItem(itemResponse("unrelated")) }
            assertNotNull(cache.view.value.items.find { it.id == "unrelated" })
        } finally { cache.close() }
    }
    @Test fun delayedItemListCommentAndProjectionRepliesCannotUndoAcknowledgedEviction() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-visibility-").toFile()
        try {
            for (kind in listOf("item", "board", "agenda", "list", "comments", "attachment")) {
                val file = root.resolve("$kind.db")
                val cache = ReadCache({ openDesktopCache(file) }, { now })
                val item = itemRead(collabId("item"))
                val list = listFixture().copy(itemId = item.id)
                val board = item.copy(occurrence = occurrenceRead("o", item.id))
                val agenda = item.copy(agenda = AgendaRead("h", "self", "o", item.id, now, startsAt = now))
                val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                val transport = object : ApiTransport {
                    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                        started.complete(Unit); release.await()
                        return when (kind) {
                            "item" -> WireJson.encodeToJsonElement(itemResponse(item.id)).jsonObject
                            "list" -> WireJson.encodeToJsonElement(list).jsonObject
                            "comments" -> buildJsonObject { put("items", WireJson.encodeToJsonElement(listOf(commentFixture()))) }
                            else -> buildJsonObject { put("items", WireJson.encodeToJsonElement(listOf(if (kind == "board") board else agenda))) }
                        }
                    }
                }
                val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
                try {
                    cache.install(emptySnapshot(1).copy(items = listOf(item), lists = listOf(list), comments = listOf(commentFixture()), attachments = listOf(attachmentFixture())))
                    val pending = async {
                        try {
                            when (kind) {
                                "item" -> data.items.get(item.id)
                                "board" -> data.board.refresh()
                                "agenda" -> data.agenda.refresh(now - 1.days, now + 1.days)
                                "list" -> data.lists.get(list.id)
                                "comments" -> data.comments.refresh(item.id)
                                else -> cache.inSession { started.complete(Unit); release.await(); cache.putAttachment(attachmentFixture()) }
                            }
                        } catch (_: StaleVisibilityReply) { /* stale reply is discarded */ }
                    }
                    started.await(); cache.evictItem(item.id); cache.acknowledgeCursor(2); release.complete(Unit); pending.await()
                    fun absent() {
                        val view = cache.view.value
                        assertTrue(view.items.isEmpty() && view.itemWrites.isEmpty() && view.board.isEmpty() && view.agenda.isEmpty() &&
                            view.lists.isEmpty() && view.comments.isEmpty() && view.attachments.isEmpty(), "$kind resurrected private rows")
                        assertTrue(item.id in view.evictedItemIds, "$kind lost tombstone")
                    }
                    absent(); cache.close()
                    val reopened = ReadCache({ openDesktopCache(file) }, { now })
                    try {
                        reopened.initialize()
                        assertTrue(reopened.view.value.items.isEmpty())
                        assertTrue(item.id in reopened.view.value.evictedItemIds, "durable eviction marker")
                        reopened.inSession { reopened.putItem(itemResponse(item.id)) }
                        assertEquals(item.id, reopened.view.value.items.single().id, "new request can restore access")
                        assertFalse(item.id in reopened.view.value.evictedItemIds)
                    } finally { reopened.close() }
                } finally { cache.close() }
            }
        } finally { root.deleteRecursively() }
    }
}
