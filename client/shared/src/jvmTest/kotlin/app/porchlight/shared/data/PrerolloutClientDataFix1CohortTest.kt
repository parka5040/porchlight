package app.porchlight.shared.data

import app.porchlight.shared.api.models.CreateRequest
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientDataFix1CohortTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private open class Server : ApiTransport {
        val snapshots = AtomicInteger()
        var hasItem = false
        var snapshot: suspend (Int) -> Unit = {}
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
            "/v1/snapshot" -> {
                val n = snapshots.incrementAndGet()
                val rows = if (hasItem) listOf(itemRead("new")) else emptyList()
                snapshot(n)
                WireJson.encodeToJsonElement(emptySnapshot(n.toLong() * 10).copy(items = rows)).jsonObject
            }
            "/v1/items" -> { hasItem = true; WireJson.encodeToJsonElement(itemResponse("new")).jsonObject }
            else -> buildJsonObject { put("items", JsonArray(emptyList())) }
        }
    }

    @Test fun followUpRenewsVisibilityAfterOlderSnapshotOmitsLocalCreation() = creation(false)
    @Test fun evictionAfterFollowUpStartsStillWins() = creation(true)
    private fun creation(evict: Boolean) = runBlocking {
        val cache = memoryCache { now }; val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val firstStaged = CompletableDeferred<Unit>(); val releaseFirst = CompletableDeferred<Unit>()
        val secondStaged = CompletableDeferred<Unit>(); val releaseSecond = CompletableDeferred<Unit>()
        val queued = CompletableDeferred<Unit>()
        data.sync.fetchListScopes = {
            if (it.cursor == 10L) { firstStaged.complete(Unit); releaseFirst.await() }
            if (it.cursor == 20L && evict) { secondStaged.complete(Unit); releaseSecond.await() }
            emptyList()
        }
        data.items.afterWrite = {
            cache.inSession {
                coroutineScope {
                    val followUp = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    queued.complete(Unit)
                    followUp.await()
                }
            }
        }
        try {
            cache.install(emptySnapshot(0))
            coroutineScope {
                val old = async { data.sync.refresh() }
                firstStaged.await()
                val create = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("new")))
                val write = async { data.items.create(create, "create-key") }
                queued.await()
                assertEquals("new", cache.view.value.items.single().id)
                releaseFirst.complete(Unit)
                if (evict) {
                    secondStaged.await(); data.sync.evictItem("new"); releaseSecond.complete(Unit)
                }
                assertIs<WriteResult.Success<*>>(write.await()); old.await()
            }
            assertEquals(20L, cache.sync.value?.cursor)
            assertEquals(if (evict) emptyList() else listOf("new"), cache.view.value.items.map { it.id },
                "The satisfying snapshot must install the new item before acknowledging its cursor; later evictions win")
            data.sync.refreshForEvent(20)
            assertEquals(if (evict) emptyList() else listOf("new"), cache.view.value.items.map { it.id })
            assertEquals(2, server.snapshots.get())
        } finally { releaseFirst.complete(Unit); releaseSecond.complete(Unit); data.close() }
    }

    @Test fun eventAndWriteFinishTheirOwnCohortsWhileNewerWorkAndEvictionContinue() = runBlocking {
        val cache = memoryCache { now }; val server = Server().apply { hasItem = true }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val starts = List(3) { CompletableDeferred<Unit>() }; val releases = List(3) { CompletableDeferred<Unit>() }
        server.snapshot = { n -> if (n <= starts.size) { starts[n - 1].complete(Unit); releases[n - 1].await() } }
        val writeQueued = CompletableDeferred<Unit>()
        data.items.afterWrite = {
            cache.inSession {
                coroutineScope {
                    val refresh = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    writeQueued.complete(Unit); refresh.await()
                }
            }
        }
        val evicted = CompletableDeferred<Unit>()
        val live = LiveSync(StreamSource { _, receive ->
            receive(StreamFrame.Tickle(10, "item.changed", "item", "new", false))
            receive(StreamFrame.Tickle(11, "item.access_changed", "item", "new", true))
        }, { cache.sync.value?.cursor }, { frame ->
            if (frame.evicted) { data.sync.evictItem(frame.entityId); evicted.complete(Unit) }
            else data.sync.refreshForEvent(frame.seq)
        }, { seq -> cache.inSession { cache.acknowledgeCursor(seq) } }, { data.sync.refresh(true) }, { now })
        try {
            cache.install(emptySnapshot(0).copy(items = listOf(itemRead("new"))))
            coroutineScope {
                val events = async { live.connectOnce() }; starts[0].await()
                cache.inSession {
                    coroutineScope {
                        val request = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("new")))
                        val write = async { data.items.create(request, "cohort-create") }
                        writeQueued.await()
                        releases[0].complete(Unit); starts[1].await()
                        val newer = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                        assertNotNull(withTimeoutOrNull(2_000) { evicted.await(); true },
                            "The first event must finish and apply the queued eviction while the next refresh is held")
                        assertTrue(cache.view.value.items.isEmpty())
                        releases[1].complete(Unit); starts[2].await()
                        val last = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                        assertIs<WriteResult.Success<*>>(withTimeoutOrNull(2_000) { write.await() },
                            "A write must finish with its cohort, not wait for the newer demand")
                        releases[2].complete(Unit); newer.await(); last.await()
                    }
                }
                events.await()
            }
        } finally { releases.forEach { it.complete(Unit) }; data.close() }
    }

    @Test fun failureIsDeliveredOnlyToTheCohortWhoseReadFailed() = runBlocking {
        val cache = memoryCache { now }; val server = Server()
        val data = HouseholdData(cache, GeneratedCall({ server }, { "session" }), { now })
        val starts = List(2) { CompletableDeferred<Unit>() }; val releases = List(2) { CompletableDeferred<Unit>() }
        server.snapshot = { n ->
            if (n <= 2) { starts[n - 1].complete(Unit); releases[n - 1].await() }
            if (n == 2) throw ApiProblem("unavailable", 500, "Unavailable")
        }
        try {
            cache.inSession {
                supervisorScope {
                    val first = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    starts[0].await()
                    val failed = async(start = CoroutineStart.UNDISPATCHED) { runCatching { data.sync.refresh() } }
                    releases[0].complete(Unit); starts[1].await()
                    val later = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refresh() }
                    withTimeout(2_000) { first.await() }
                    releases[1].complete(Unit)
                    assertIs<ApiProblem>(failed.await().exceptionOrNull())
                    later.await()
                }
            }
            assertEquals(30L, cache.sync.value?.cursor)
        } finally { releases.forEach { it.complete(Unit) }; data.close() }
    }

    @Test fun refreshAfterCloseCancelsInsteadOfOrphaningAWaiter() = runBlocking {
        val data = HouseholdData(memoryCache { now }, GeneratedCall({ Server() }, { "session" }), { now })
        data.close()
        assertIs<CancellationException>(withTimeout(2_000) { runCatching { data.sync.refresh() }.exceptionOrNull() })
        Unit
    }

    @Test fun replacementTokenBindingDoesNotInheritObsoleteFlightCancellation() = runBlocking {
        val oldStarted = CompletableDeferred<Unit>(); val releaseOld = CompletableDeferred<Unit>()
        val newStarted = CompletableDeferred<Unit>(); val streamStarted = CompletableDeferred<Unit>()
        val tokens = mutableListOf<String?>()
        val server = object : Server(), TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/me" -> buildJsonObject { put("member_id", "member") }
                "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
                "/v1/auth/login" -> buildJsonObject { put("session", buildJsonObject { put("token", "replacement"); put("expires_at", "2027-01-01T00:00:00Z") }) }
                "/v1/snapshot" -> {
                    synchronized(tokens) { tokens.add(session) }
                    if (session == "old") { oldStarted.complete(Unit); releaseOld.await() } else newStarted.complete(Unit)
                    WireJson.encodeToJsonElement(emptySnapshot(if (session == "old") 1 else 20).copy(items = listOf(itemRead(if (session == "old") "obsolete" else "current")))).jsonObject
                }
                else -> super.request(method, path, body, session, idempotencyKey, version, stepUp)
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) {
                assertEquals("replacement", session)
                streamStarted.complete(Unit)
                receive(StreamFrame.Tickle(21, "item.access_changed", "item", "current", true))
                awaitCancellation()
            }
        }
        val keys = object : DeviceKeyStore {
            var value = DeviceTrust("device", "leaf", "root", "old", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z", "member")
            override suspend fun load() = value
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
            override suspend fun clear() = Unit
            override suspend fun csr() = "CSR"
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val cache = memoryCache { now }; val controller = TrustController(keys, server, "desktop", { now }, false)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        try {
            data.bindTrust(controller, scope); controller.restore(); oldStarted.await()
            val epoch = controller.sessionEpoch.value
            controller.login("fixture-password")
            assertEquals(epoch, controller.sessionEpoch.value)
            cache.inSession {
                supervisorScope {
                    val current = async(start = CoroutineStart.UNDISPATCHED) {
                        runCatching { withBackgroundSession(controller) { data.sync.refresh() } }
                    }
                    releaseOld.complete(Unit)
                    val result = current.await()
                    assertTrue(result.isSuccess, "The replacement session must not inherit the old reply's stale-session cancellation: ${result.exceptionOrNull()?.javaClass?.simpleName}")
                }
            }
            withTimeout(5_000) { newStarted.await(); streamStarted.await(); cache.sync.first { it?.cursor == 21L } }
            assertTrue(cache.view.value.items.isEmpty(), "The replacement stream must process its eviction")
            assertTrue(tokens.contains("replacement"))
            assertTrue(cache.view.value.items.none { it.id == "obsolete" })
        } finally { releaseOld.complete(Unit); scope.cancel(); controller.close(); data.close() }
    }
}
