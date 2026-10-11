package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.live.DeliveryInventory
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class AuditClientDataFix1VisibilityTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private val item = collabId("item")
    @Test fun notificationAndPlanningRepliesCannotCrossItemOrOccurrenceEviction() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-derived-").toFile()
        try {
            for (kind in listOf("notification", "planning", "planning-write")) for (parent in listOf("item", "occurrence")) {
                val file = root.resolve("$kind-$parent.db")
                val cache = ReadCache({ openDesktopCache(file) }, { now })
                val notification = Notification("n", "activity", "activity", "item", item, 1,
                    NotificationSummary("private", "private", "private", "activity", false), now, now, 1,
                    itemId = item, occurrenceId = "o", releasedAt = now)
                val suggestion = SuggestionResponse("s", "self", item, "o", now, now + 1.days, SuggestionStatus.proposed, "run", 1, now, emptyList(), 1)
                val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                val transport = object : ApiTransport {
                    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                        started.complete(Unit); release.await()
                        val row = if (kind == "notification") WireJson.encodeToJsonElement(notification) else WireJson.encodeToJsonElement(suggestion)
                        return if (kind == "planning-write") row.jsonObject else buildJsonObject { put("items", JsonArray(listOf(row))) }
                    }
                }
                val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
                data.planning.afterAccept = {}
                try {
                    cache.install(emptySnapshot(1).copy(items = listOf(itemRead(item)), occurrences = listOf(occurrenceRead("o", item))))
                    val pending = async {
                        try { when (kind) {
                            "notification" -> data.notifications.refresh()
                            "planning" -> data.planning.refresh()
                            else -> data.planning.dismiss(suggestion)
                        } } catch (_: StaleVisibilityReply) { }
                    }
                    started.await()
                    if (parent == "item") cache.evictItem(item) else cache.evictOccurrence("o")
                    cache.acknowledgeCursor(2)
                    release.complete(Unit); pending.await()
                    fun absent(cache: ReadCache) {
                        assertTrue(cache.view.value.notifications.isEmpty(), "$kind resurrected private notification after $parent eviction")
                        assertTrue(cache.view.value.suggestions.isEmpty(), "$kind resurrected suggestion after $parent eviction")
                        assertTrue(DeliveryInventory().serverToPost(cache.view.value).isEmpty(), "private content must not be posted")
                    }
                    absent(cache); cache.close()
                    val reopened = ReadCache({ openDesktopCache(file) }, { now })
                    try { reopened.initialize(); absent(reopened) } finally { reopened.close() }
                } finally { release.complete(Unit); cache.close() }
            }
        } finally { root.deleteRecursively() }
    }

    @Test fun successfulDeletedRepresentationFencesEarlierRealItemGet() = runTest {
        val cache = memoryCache { now }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") { started.complete(Unit); release.await() }
                return WireJson.encodeToJsonElement(itemResponse(item).copy(version = if (method == "GET") 1 else 2,
                    deletedAt = if (method == "GET") null else now)).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(item))))
            val pending = async { try { data.items.get(item) } catch (_: StaleVisibilityReply) { } }
            started.await(); assertIs<WriteResult.Success<*>>(data.items.delete(item, 1))
            release.complete(Unit); pending.await()
            assertTrue(cache.view.value.items.isEmpty(), "older GET resurrected successfully deleted item")
            assertTrue(cache.view.value.itemWrites.isEmpty())
        } finally { release.complete(Unit); cache.close() }
    }

    @Test fun snapshotAndCoveredWindowOmissionsFenceOldChildReplies() = runTest {
        for (kind in listOf("comment", "attachment", "occurrence", "window")) {
            val cache = memoryCache { now }
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            try {
                val occurrence = occurrenceRead("o", item).copy(startsAt = now)
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead(item)), comments = listOf(commentFixture()),
                    attachments = listOf(attachmentFixture()), occurrences = listOf(occurrence)))
                val old = async {
                    try { cache.inSession {
                        started.complete(Unit); release.await()
                        when (kind) {
                            "comment" -> cache.putComments(listOf(commentFixture()))
                            "attachment" -> cache.putAttachment(attachmentFixture())
                            else -> cache.putOccurrence(occurrenceResponse("o", item).copy(startsAt = now))
                        }
                    } } catch (_: StaleVisibilityReply) { }
                }
                started.await()
                if (kind == "window") cache.putOccurrences(item, emptyList(), now - 1.days, now + 1.days)
                else cache.install(emptySnapshot(2).copy(items = listOf(itemRead(item))))
                release.complete(Unit); old.await()
                val view = cache.view.value
                assertTrue(when (kind) {
                    "comment" -> view.comments.isEmpty()
                    "attachment" -> view.attachments.isEmpty()
                    else -> view.occurrences.isEmpty()
                }, "$kind reply resurrected authoritatively removed child")
            } finally { release.complete(Unit); cache.close() }
        }
    }
}
