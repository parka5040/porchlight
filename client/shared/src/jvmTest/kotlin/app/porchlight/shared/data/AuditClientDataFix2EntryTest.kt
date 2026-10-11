package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataFix2EntryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun archivedAndOmittedEntriesFenceDelayedEntryListAndSnapshotReplies() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-entry-").toFile()
        try {
            for (removal in listOf("archive", "clear", "refetch", "snapshot")) for (reply in listOf("entry", "list", "snapshot")) {
                val file = root.resolve("$removal-$reply.db")
                val cache = ReadCache({ openDesktopCache(file) }, { now })
                val entry = entryFixture("private entry", "a")
                val list = listFixture().copy(propertyEntries = listOf(entry))
                val empty = list.copy(version = 2, propertyEntries = emptyList(), gotIt = emptyList())
                val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                var removing = false
                var removed = false
                val transport = object : ApiTransport {
                    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                        if (removing) return when (removal) {
                            "archive" -> WireJson.encodeToJsonElement(entry.copy(version = 2, archivedAt = now)).jsonObject
                            else -> WireJson.encodeToJsonElement(empty).jsonObject
                        }
                        if (removed) throw IllegalStateException("offline follow-up")
                        started.complete(Unit); release.await()
                        return if (reply == "entry") WireJson.encodeToJsonElement(entry).jsonObject else WireJson.encodeToJsonElement(list).jsonObject
                    }
                }
                val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
                data.lists.afterWrite = { throw IllegalStateException("offline follow-up") }
                try {
                    cache.inSession { cache.install(it, emptySnapshot(1).copy(lists = listOf(list))) }
                    val pending = async {
                        try { when (reply) {
                            "entry" -> data.lists.check(entry.id, 1, true)
                            "list" -> data.lists.get(list.id)
                            else -> cache.inSession { fence -> started.complete(Unit); release.await(); cache.install(fence, emptySnapshot(1).copy(lists = listOf(list))) }
                        } } catch (_: StaleVisibilityReply) { }
                    }
                    started.await(); removing = true
                    when (removal) {
                        "archive" -> data.lists.archive(entry)
                        "clear" -> data.lists.clear(list)
                        "refetch" -> data.lists.get(list.id)
                        else -> cache.inSession { cache.install(it, emptySnapshot(2).copy(lists = listOf(empty))) }
                    }
                    removing = false; removed = true
                    release.complete(Unit); pending.await()
                    fun absent(c: ReadCache) = assertTrue(c.view.value.lists.all { it.propertyEntries.isEmpty() && it.gotIt.isEmpty() }, "$removal/$reply resurrected archived entry")
                    absent(cache); cache.close()
                    val reopened = ReadCache({ openDesktopCache(file) }, { now })
                    try {
                        reopened.initialize(); absent(reopened)
                        // A genuinely fresh authorized read can restore a key; unrelated keys are not fenced.
                        reopened.inSession { reopened.putList(it, list.copy(version = 3)) }
                        assertEquals(entry.id, reopened.view.value.lists.single().propertyEntries.single().id)
                    } finally { reopened.close() }
                } finally { release.complete(Unit); cache.close() }
            }
        } finally { root.deleteRecursively() }
    }
}
