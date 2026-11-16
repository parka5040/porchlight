package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientDataPerformanceTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun driver() = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY).also { PorchlightDb.Schema.create(it) }

    @Test fun cursorAcknowledgmentReadsZeroPayloadRowsAndDoesNotPublish() = runBlocking {
        val delegate = driver()
        var payloadRows = 0
        val counted = object : SqlDriver by delegate {
            override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> =
                delegate.executeQuery(identifier, sql, { cursor ->
                    mapper(object : SqlCursor by cursor {
                        override fun next(): QueryResult<Boolean> = cursor.next().also {
                            if (it.value && Regex("FROM Cache(Item|Member|Settings|Occurrence|Board|Agenda|Reminder|Write|Notification|NotificationPrefs|Planning|List|Comment|Attachment)\\b", RegexOption.IGNORE_CASE).containsMatchIn(sql)) payloadRows++
                        }
                    })
                }, parameters, binders)
        }
        val cache = ReadCache(counted) { now }
        try {
            cache.install(emptySnapshot(1).copy(items = List(100) { itemRead("item-$it") }))
            val before = cache.view.value
            payloadRows = 0
            cache.inSession { cache.acknowledgeCursor(2) }
            println("cursor ack payload rows=$payloadRows; presentation changed=${before !== cache.view.value}")
            assertEquals(0, payloadRows, "Cursor-only acknowledgment must not decode payload rows")
            assertSame(before, cache.view.value, "Cursor-only acknowledgment must not publish presentation state")
            assertEquals(2L, cache.sync.value?.cursor)
            cache.acknowledgeCursor(1)
            assertEquals(2L, cache.sync.value?.cursor)
        } finally { cache.close() }
    }

    @Test fun snapshotOmissionChecksVisitIncomingIdsLinearly() = runBlocking {
        val cache = memoryCache { now }
        try {
            val rows = List(400) { listFixture().copy(id = "list-$it", propertyEntries = emptyList(), gotIt = emptyList()) }
            cache.install(emptySnapshot(1).copy(lists = rows))
            var visits = 0
            val counted = object : AbstractList<app.porchlight.shared.api.models.ListResponse>() {
                override val size = rows.size
                override fun get(index: Int) = rows[index].also { visits++ }
            }
            cache.install(emptySnapshot(2).copy(lists = counted))
            println("snapshot omission incoming-ID visits=$visits for ${rows.size} lists")
            assertTrue(visits < rows.size * 10, "Snapshot omission must be linear; visited $visits incoming rows")
        } finally { cache.close() }
    }

    @Test fun generatedDecodeRunsOffTheCallerDispatcher() = runBlocking {
        Executors.newSingleThreadExecutor { Thread(it, "fixture-ui") }.asCoroutineDispatcher().use { caller ->
            val call = GeneratedCall({ error("No network") }, { null })
            withContext(caller) {
                val callerThread = Thread.currentThread()
                val decoded = call.decode<DecodedThread>(buildJsonObject {})
                assertNotSame(callerThread, decoded.thread, "JSON model decoding must leave the caller dispatcher")
                assertSame(callerThread, Thread.currentThread(), "UI continuation must return to its dispatcher")
            }
        }
    }

    @Test fun twentyOverlappingRefreshesCoalesceToAtMostTwo() = runBlocking {
        val cache = memoryCache { now }
        val snapshots = AtomicInteger()
        val requests = AtomicInteger()
        val started = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                requests.incrementAndGet()
                return if (path == "/v1/snapshot") {
                    if (snapshots.incrementAndGet() == 1) { started.complete(Unit); release.await() }
                    WireJson.encodeToJsonElement(emptySnapshot(20)).jsonObject
                } else buildJsonObject { put("items", JsonArray(emptyList())) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
        try {
            cache.inSession {
                coroutineScope {
                    val first = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    started.await()
                    val rest = List(19) { async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() } }
                    release.complete(Unit)
                    (rest + first).awaitAll()
                }
            }
            println("20 overlapping demands: snapshots=${snapshots.get()}, requests=${requests.get()}")
            assertTrue(snapshots.get() <= 2, "20 overlapping demands must cost at most two refreshes; got ${snapshots.get()}")
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun databaseDispatcherIsSerializedAndIoBacked() {
        val source = Files.readString(Path.of("src/commonMain/kotlin/app/porchlight/shared/data/ReadCache.kt"))
        assertTrue(source.contains("Dispatchers.IO.limitedParallelism(1)"), "Blocking SQLite needs a serialized IO-backed dispatcher")
        assertFalse(source.contains("withContext(Dispatchers.Default)"), "SQLite must not compete with CPU decoding")
    }

    @Test fun cachePredicatesUseIndexesAndSchemaVersionAdvances() {
        val db = driver()
        try {
            val queries = listOf(
                "DELETE FROM CacheNotification WHERE item_id = 'item'",
                "DELETE FROM CachePlanning WHERE item_id = 'item'",
                "SELECT generation FROM CacheVisibility ORDER BY generation DESC LIMIT 1 OFFSET 256",
                "SELECT max(generation) FROM CacheVisibility",
                "DELETE FROM CacheVisibility WHERE generation <= 10",
                "DELETE FROM CacheOccurrence WHERE expires_at < 10",
                "DELETE FROM CacheAgenda WHERE expires_at < 10",
                "DELETE FROM CacheReminder WHERE expires_at < 10",
            )
            for (sql in queries) {
                val plans = db.executeQuery(null, "EXPLAIN QUERY PLAN $sql", { cursor ->
                    val rows = mutableListOf<String>()
                    while (cursor.next().value) rows += checkNotNull(cursor.getString(3))
                    QueryResult.Value(rows)
                }, 0).value
                assertTrue(plans.any { "INDEX" in it } && plans.none { "TEMP B-TREE" in it }, "Unindexed predicate: $sql: $plans")
            }
            assertTrue(PorchlightDb.Schema.version > 10, "Disposable schema must advance for new indexes")
        } finally { db.close() }
    }
}

@kotlinx.serialization.Serializable(with = DecodedThreadSerializer::class)
private data class DecodedThread(val thread: Thread)
private object DecodedThreadSerializer : KSerializer<DecodedThread> {
    override val descriptor = PrimitiveSerialDescriptor("DecodedThread", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): DecodedThread {
        (decoder as JsonDecoder).decodeJsonElement()
        return DecodedThread(Thread.currentThread())
    }
    override fun serialize(encoder: Encoder, value: DecodedThread) = error("Not encoded")
}
