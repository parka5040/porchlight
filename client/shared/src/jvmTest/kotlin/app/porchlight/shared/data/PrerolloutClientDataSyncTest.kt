package app.porchlight.shared.data

import app.porchlight.shared.live.LiveSync
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.live.StreamSource
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientDataSyncTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Server : ApiTransport {
        val snapshots = AtomicInteger()
        val requests = AtomicInteger()
        val active = AtomicInteger()
        val maxActive = AtomicInteger()
        var cursor = 20L
        var snapshotGate: (suspend () -> Unit)? = null
        var projectionGate: (suspend () -> Unit)? = null
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
            requests.incrementAndGet()
            return when (path.substringBefore('?')) {
                "/v1/snapshot" -> {
                    snapshots.incrementAndGet()
                    snapshotGate?.invoke()
                    WireJson.encodeToJsonElement(emptySnapshot(cursor).copy(items = listOf(itemRead("one")))).jsonObject
                }
                "/v1/items/one", "/v1/items/one/reminders" -> WireJson.encodeToJsonElement(itemResponse("one")).jsonObject
                else -> {
                    maxActive.accumulateAndGet(active.incrementAndGet(), ::maxOf)
                    try { projectionGate?.invoke(); buildJsonObject { put("items", JsonArray(emptyList())) } }
                    finally { active.decrementAndGet() }
                }
            }
        }
    }
    private fun live(cache: ReadCache, data: HouseholdData, frames: List<StreamFrame>) = LiveSync(
        StreamSource { _, receive -> frames.forEach { receive(it) } }, { cache.sync.value?.cursor },
        { data.sync.refresh() }, { seq -> cache.inSession { cache.acknowledgeCursor(seq) } },
        { data.sync.refresh(authoritative = true) }, { now })

    @Test fun localEditAndItsQueuedStreamEchoCostOneRefresh() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            assertIs<WriteResult.Success<*>>(data.items.setReminders("one", 1, listOf(15), "fixture-key"))
            val afterWrite = server.requests.get()
            live(cache, data, listOf(StreamFrame.Tickle(20, "item.changed", "item", "one", false))).connectOnce()
            println("edit + own echo: requests=${server.requests.get()}, refreshes=${server.snapshots.get()}")
            assertEquals(4, afterWrite)
            assertEquals(afterWrite, server.requests.get(), "The echo already covered by the snapshot must not refetch")
            assertEquals(1, server.snapshots.get())
        } finally { data.close() }
    }

    @Test fun localEditAndItsInFlightEchoCostOneRefresh() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.snapshotGate = { started.complete(Unit); release.await() }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            coroutineScope {
                val write = async { data.items.setReminders("one", 1, listOf(15), "fixture-key") }
                started.await()
                cache.inSession {
                    coroutineScope {
                        val echo = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(20) }
                        release.complete(Unit)
                        echo.await()
                    }
                }
                assertIs<WriteResult.Success<*>>(write.await())
            }
            println("edit + in-flight echo: requests=${server.requests.get()}, refreshes=${server.snapshots.get()}")
            assertEquals(1, server.snapshots.get(), "An own-write receipt is not a visibility eviction")
            assertEquals(4, server.requests.get())
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun twentyQueuedItemEventsAreCoveredByOneSnapshot() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.install(emptySnapshot(0))
            live(cache, data, (1L..20L).map { StreamFrame.Tickle(it, "item.changed", "item", "one", false) }).connectOnce()
            println("20 queued item events: requests=${server.requests.get()}, refreshes=${server.snapshots.get()}")
            assertTrue(server.snapshots.get() <= 2)
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { data.close() }
    }

    @Test fun inFlightEchoSharesRefreshAndWaiterCancellationDoesNotCancelIt() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.snapshotGate = { started.complete(Unit); release.await() }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.inSession {
                coroutineScope {
                    val write = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    started.await()
                    val echo = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(20) }
                    write.cancelAndJoin()
                    release.complete(Unit)
                    echo.await()
                }
            }
            assertEquals(1, server.snapshots.get())
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun independentProjectionsOverlapButNeverExceedThreeRequests() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val three = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.projectionGate = { if (server.active.get() >= 3) three.complete(Unit); release.await() }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        data.sync.fetchListScopes = {
            server.request("GET", "/v1/lists/retained", null, null, null, null, null)
            emptyList()
        }
        try {
            coroutineScope {
                val refresh = async { data.sync.refresh() }
                withTimeout(5_000) { three.await() }
                assertEquals(3, server.maxActive.get(), "Board, agenda and retained scopes should overlap")
                val others = List(3) { async { data.sync.refreshProjections() } }
                release.complete(Unit)
                (others + refresh).awaitAll()
            }
            assertEquals(3, server.maxActive.get(), "All sync projection reads share the same three slots")
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun oldEpochRefreshCannotJoinOrPopulateReplacementIdentity() = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        var epoch = 0L
        cache.sessionEpoch = { epoch }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.snapshotGate = { if (server.snapshots.get() == 1) { started.complete(Unit); release.await() } }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            supervisorScope {
                val old = async { runCatching { data.sync.refresh() } }
                started.await()
                epoch++
                cache.clear()
                val replacement = async { data.sync.refresh() }
                release.complete(Unit)
                assertIs<CancellationException>(old.await().exceptionOrNull())
                replacement.await()
            }
            assertEquals(2, server.snapshots.get())
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun coalescedFollowUpKeepsTheLaterRequestsVisibilityFence() = laterFence(false)
    @Test fun inFlightEventKeepsItsLaterVisibilityFence() = laterFence(true)
    private fun laterFence(event: Boolean) = runBlocking {
        val cache = memoryCache { now }
        val server = Server()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.snapshotGate = { if (server.snapshots.get() == 1) { started.complete(Unit); release.await() } }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            coroutineScope {
                val first = async { data.sync.refresh(authoritative = true) }
                started.await()
                cache.evictItem("one")
                cache.inSession {
                    coroutineScope {
                        val later = async(start = CoroutineStart.UNDISPATCHED) {
                            if (event) data.sync.refreshForEvent(20) else data.sync.refresh()
                        }
                        release.complete(Unit)
                        later.await()
                    }
                }
                first.await()
            }
            assertEquals(listOf("one"), cache.view.value.items.map { it.id }, "A new authorized request may restore access after eviction")
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun cursorAckCannotCrossEpochAndSnapshotMarksUseAckedCursor() = runBlocking {
        val cache = memoryCache { now }
        var epoch = 0L
        cache.sessionEpoch = { epoch }
        try {
            cache.install(emptySnapshot(1))
            cache.inSession {
                epoch++
                assertFailsWith<CancellationException> { cache.acknowledgeCursor(99) }
            }
            assertEquals(1L, cache.sync.value?.cursor)
            cache.acknowledgeCursor(2)
            assertEquals(2L, cache.snapshotMark().cursor)
            cache.initialize()
            assertEquals(2L, cache.sync.value?.cursor, "Cursor-only updates must be durable")
        } finally { cache.close() }
    }
}
