package app.porchlight.shared.data

import app.porchlight.shared.api.models.CreateRequest
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlin.coroutines.CoroutineContext
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientDataFix1EventRetryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Server : ApiTransport {
        val snapshots = AtomicInteger(); val mutations = AtomicInteger()
        var overload: ApiProblem? = null
        var projectionOverload: ApiProblem? = null
        var failures = 1
        var cursor: (Int) -> Long = { 20L }
        var snapshotGate: suspend () -> Unit = {}
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
            "/v1/snapshot" -> {
                val attempt = snapshots.incrementAndGet()
                snapshotGate()
                if (attempt <= failures) overload?.let { throw it }
                WireJson.encodeToJsonElement(emptySnapshot(cursor(attempt)).copy(items = listOf(itemRead("new")))).jsonObject
            }
            "/v1/items" -> { mutations.incrementAndGet(); WireJson.encodeToJsonElement(itemResponse("new")).jsonObject }
            "/v1/board" -> {
                projectionOverload?.let { projectionOverload = null; throw it }
                buildJsonObject { put("items", JsonArray(emptyList())) }
            }
            else -> buildJsonObject { put("items", JsonArray(emptyList())) }
        }
    }
    private fun live(cache: ReadCache, data: HouseholdData, frames: List<StreamFrame>) = LiveSync(
        StreamSource { _, receive -> frames.forEach { receive(it) } }, { cache.sync.value?.cursor },
        { data.sync.refreshForEvent(it.seq) }, { seq -> cache.inSession { cache.acknowledgeCursor(seq) } },
        { data.sync.refresh(true) }, { now })

    @Test fun queuedEchoAndTwentyQueuedEventsUseEventRefreshPath() = runBlocking {
        val cache = memoryCache { now }; val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        try {
            cache.install(emptySnapshot(0))
            val request = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("new")))
            assertIs<WriteResult.Success<*>>(data.items.create(request, "fixture-create"))
            data.sync.refreshForEvent(20) // The queued own echo takes the production cursor-covered fast path.
            live(cache, data, (1L..20L).map { StreamFrame.Tickle(it, "item.changed", "item", "new", false) }).connectOnce()
            assertEquals(1, server.snapshots.get(), "The queued own echo and covered burst need no second snapshot")
            cache.install(emptySnapshot(0))
            live(cache, data, (1L..20L).map { StreamFrame.Tickle(it, "derived", "item", "new", false) }).connectOnce()
            assertEquals(2, server.snapshots.get(), "The uncovered burst must use refreshForEvent, then skip covered events")
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { data.close() }
    }

    @Test fun twentyOverlappingEventDemandsShareAtMostTwoRefreshes() = runBlocking {
        val cache = memoryCache { now }; val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        server.snapshotGate = { started.complete(Unit); release.await() }
        try {
            cache.install(emptySnapshot(0))
            cache.inSession {
                coroutineScope {
                    val first = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(1) }
                    started.await()
                    val burst = (2L..20L).map { seq -> async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(seq) } }
                    release.complete(Unit); (burst + first).awaitAll()
                }
            }
            assertTrue(server.snapshots.get() <= 2)
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun delayedUncoveredEchoJoinsTheAlreadyStartedSatisfyingCohort() = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply { cursor = { if (it == 1) 10L else 20L } }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val starts = List(2) { CompletableDeferred<Unit>() }; val releases = List(2) { CompletableDeferred<Unit>() }
        server.snapshotGate = {
            val n = server.snapshots.get()
            if (n <= 2) { starts[n - 1].complete(Unit); releases[n - 1].await() }
        }
        val resumes = Channel<Runnable>(Channel.UNLIMITED)
        val paused = object : CoroutineDispatcher() {
            override fun dispatch(context: CoroutineContext, block: Runnable) { check(resumes.trySend(block).isSuccess) }
        }
        try {
            cache.inSession {
                coroutineScope {
                    val first = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    starts[0].await()
                    val echo = async(paused, start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(20) }
                    val followUp = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    releases[0].complete(Unit); starts[1].await(); first.await()
                    // Hold this event's continuation until a read started AFTER its original demand is outstanding.
                    withTimeout(2_000) { resumes.receive() }.run()
                    releases[1].complete(Unit)
                    withTimeout(2_000) { resumes.receive() }.run()
                    echo.await(); followUp.await()
                }
            }
            assertEquals(2, server.snapshots.get(), "An uncovered echo must observe the satisfying follow-up, not force a third read")
        } finally { releases.forEach { it.complete(Unit) }; data.close(); resumes.close() }
    }

    @Test fun snapshot429RetriesOnceAfterAdvertisedDelayWithoutReplayingCreate() = retry(429, 1250, 1250)
    @Test fun snapshot503RetryDelayIsCappedAtFiveSeconds() = retry(503, 60_000, 5000)
    private fun retry(status: Int, advertised: Long, expected: Long) = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply { overload = ApiProblem("overloaded", status, "Busy", retryAfterMillis = advertised) }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val waits = mutableListOf<Long>()
        data.sync.waitForReadRetry = { waits.add(it) }
        try {
            val request = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("new")))
            val saved = assertIs<WriteResult.Success<*>>(data.items.create(request, "fixture-create"))
            assertFalse(saved.refreshPending, "The snapshot read must recover from one advertised overload")
            assertEquals(listOf(expected), waits)
            assertEquals(2, server.snapshots.get()); assertEquals(1, server.mutations.get())
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { data.close() }
    }

    @Test fun proxy503RetryAfterHeaderReachesTheReadScheduler() = runBlocking {
        val snapshots = AtomicInteger()
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            if (request.url.encodedPath == "/v1/snapshot") {
                if (snapshots.incrementAndGet() == 1) respond("Busy", HttpStatusCode.ServiceUnavailable,
                    headersOf(HttpHeaders.ContentType to listOf("text/plain"), HttpHeaders.RetryAfter to listOf("2")))
                else respond(WireJson.encodeToJsonElement(emptySnapshot(20)).toString(),
                    headers = headersOf(HttpHeaders.ContentType, "application/json"))
            } else respond("{\"items\":[]}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
        val waits = mutableListOf<Long>(); data.sync.waitForReadRetry = { waits.add(it) }
        try {
            data.sync.refresh()
            assertEquals(listOf(2000L), waits); assertEquals(2, snapshots.get())
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { data.close(); transport.close() }
    }

    @Test fun projectionOverloadRetriesTheStagedReadBeforePublishingItsCursor() = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply {
            projectionOverload = ApiProblem("overloaded", 503, "Busy", retryAfterMillis = 250)
        }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val waits = mutableListOf<Long>()
        data.sync.waitForReadRetry = { assertNull(cache.sync.value); waits.add(it) }
        try {
            data.sync.refresh()
            assertEquals(listOf(250L), waits); assertEquals(2, server.snapshots.get())
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { data.close() }
    }

    @Test fun secondOverloadFailsWithoutAnotherRetry() = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply {
            overload = ApiProblem("overloaded", 503, "Busy", retryAfterMillis = 1000); failures = 3
        }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val waits = mutableListOf<Long>(); data.sync.waitForReadRetry = { waits.add(it) }
        try {
            assertFailsWith<ApiProblem> { data.sync.refresh() }
            assertEquals(listOf(1000L), waits); assertEquals(2, server.snapshots.get()); assertNull(cache.sync.value)
        } finally { data.close() }
    }

    @Test fun overloadWithoutRetryAfterAndOtherProblemsFailWithoutRetry() = runBlocking {
        for ((status, retry) in listOf(429 to null, 503 to null, 500 to 1000L, 403 to 1000L)) {
            val cache = memoryCache { now }; val server = Server().apply { overload = ApiProblem("failure", status, "Failure", retryAfterMillis = retry) }
            val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
            data.sync.waitForReadRetry = { fail("No retry authorized") }
            try { assertFailsWith<ApiProblem> { data.sync.refresh() }; assertEquals(1, server.snapshots.get()) }
            finally { data.close() }
        }
    }

    @Test fun closingDataCancelsAdvertisedRetryDelay() = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply { overload = ApiProblem("overloaded", 503, "Busy", retryAfterMillis = 1000) }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val waiting = CompletableDeferred<Unit>()
        data.sync.waitForReadRetry = { waiting.complete(Unit); awaitCancellation() }
        try {
            supervisorScope {
                val refresh = async { runCatching { data.sync.refresh() } }
                assertNotNull(withTimeoutOrNull(2_000) { waiting.await(); true }, "The read must enter its cancellable retry delay")
                data.close()
                assertIs<CancellationException>(withTimeout(2_000) { refresh.await().exceptionOrNull() })
            }
            assertEquals(1, server.snapshots.get())
        } finally { data.close() }
    }
}
