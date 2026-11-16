package app.porchlight.shared.data

import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientSyncAdmissionTest {
    private val now = Instant.parse("2026-11-01T12:00:00Z")

    @Test fun restoredAccessDemandCannotJoinReadBetweenFenceCaptureAndActivation() = runBlocking {
        val cache = memoryCache { now }
        val reads = AtomicInteger()
        val server = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject =
                if (path.substringBefore('?') == "/v1/snapshot") WireJson.encodeToJsonElement(
                    emptySnapshot(reads.incrementAndGet() * 10L).copy(items = listOf(itemRead("restored")))).jsonObject
                else buildJsonObject { put("items", JsonArray(emptyList())) }
        }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "fixture-session" }), { now })
        val captured = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        var first = true
        cache.freshVisibilityCaptured = {
            if (first) { first = false; captured.complete(Unit); release.await() }
        }
        val enqueued = CompletableDeferred<Unit>()
        val admitted = CompletableDeferred<Unit>()
        data.sync.refreshDemandAdmitted = { admitted.complete(Unit) }
        val live = LiveSync(StreamSource { _, receive ->
            receive(StreamFrame.Tickle(10, "item.access_changed", "item", "restored", false))
        }, { cache.sync.value?.cursor }, { frame ->
            coroutineScope {
                val demand = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(frame.seq) }
                enqueued.complete(Unit)
                demand.await()
            }
        },
            { seq -> cache.inSession { cache.acknowledgeCursor(seq) } }, { data.sync.refresh(true) }, { now })
        try {
            cache.install(emptySnapshot(0).copy(items = listOf(itemRead("restored"))))
            withTimeout(5_000) {
                val older = async { data.sync.refresh() }
                captured.await()
                data.sync.evictItem("restored")
                // Inherit a newly captured post-eviction lease, then enter admission undispatched.
                cache.inSession {
                    val restored = async(start = CoroutineStart.UNDISPATCHED) { live.connectOnce() }
                    enqueued.await()
                    // Before the fix admission succeeds in this gap; atomic activation instead blocks it.
                    withTimeoutOrNull(250) { admitted.await() }
                    release.complete(Unit)
                    older.await(); restored.await()
                }
            }
            assertEquals(listOf("restored"), cache.view.value.items.map { it.id },
                "A restored-access event must not be acknowledged with its item absent behind a pre-eviction fence")
            assertEquals(2, reads.get(), "The later-visibility demand requires its own authorized read")
            assertEquals(20L, cache.sync.value?.cursor)
        } finally { release.complete(Unit); data.close() }
    }

    @Test fun activationLockIsReleasedBeforeNetworkAndPostStartEvictionStillWins() = runBlocking {
        val cache = memoryCache { now }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val reads = AtomicInteger()
        val server = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject =
                if (path.substringBefore('?') == "/v1/snapshot") {
                    val n = reads.incrementAndGet()
                    if (n == 1) { started.complete(Unit); release.await() }
                    WireJson.encodeToJsonElement(emptySnapshot(n * 10L).copy(items = listOf(itemRead("removed")))).jsonObject
                } else buildJsonObject { put("items", JsonArray(emptyList())) }
        }
        val data = HouseholdData(cache, GeneratedCall({ server }, { "fixture-session" }), { now })
        val admitted = CompletableDeferred<Unit>()
        data.sync.refreshDemandAdmitted = { admitted.complete(Unit) }
        try {
            cache.install(emptySnapshot(0).copy(items = listOf(itemRead("removed"))))
            withTimeout(5_000) {
                val older = async { data.sync.refresh() }; started.await()
                // Admission must remain available during the network read; eviction is never queued behind it.
                cache.inSession {
                    val echo = async(start = CoroutineStart.UNDISPATCHED) { data.sync.refreshForEvent(10) }
                    withTimeout(2_000) { admitted.await() }
                    data.sync.evictItem("removed")
                    release.complete(Unit); older.await(); echo.await()
                }
            }
            assertTrue(cache.view.value.items.isEmpty())
            assertEquals(1, reads.get())
        } finally { release.complete(Unit); data.close() }
    }
}
