package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientSyncFixGCohortTest {
    @Test fun stagedInstallRejectsReplacementSessionWithoutCollectorCancellation() = runBlocking {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val revision = AtomicLong(0)
        val cache = memoryCache { now }.also { it.sessionRevision = revision::get }
        val staged = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        try {
            cache.install(emptySnapshot(0))
            supervisorScope {
                val install = async { runCatching { cache.inSession { fence ->
                    staged.complete(Unit); release.await()
                    cache.install(fence, emptySnapshot(20).copy(items = listOf(itemRead("obsolete"))))
                } } }
                staged.await(); revision.incrementAndGet(); release.complete(Unit)
                assertIs<CancellationException>(install.await().exceptionOrNull(), "SQL admission must independently fence a replacement session")
                assertEquals(0L, cache.sync.value?.cursor)
                assertTrue(cache.view.value.items.isEmpty())
            }
        } finally { release.complete(Unit); cache.close() }
    }

    @Test fun promotedFailedFlightRemainsInStartupBarrier() = failedFlight(false)
    @Test fun promotedFailedFlightIsCancelledOnSessionReplacement() = failedFlight(true)
    private fun failedFlight(replace: Boolean) = runBlocking {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val revision = AtomicLong(0); val reads = AtomicInteger()
        val firstStarted = CompletableDeferred<Unit>(); val failFirst = CompletableDeferred<Unit>()
        val secondStaged = CompletableDeferred<Unit>(); val releaseSecond = CompletableDeferred<Unit>(); val secondEnded = CompletableDeferred<Unit>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path.substringBefore('?') == "/v1/snapshot") {
                    if (reads.incrementAndGet() == 1) { firstStarted.complete(Unit); failFirst.await(); throw ApiProblem("unavailable", 500, "Unavailable") }
                    return WireJson.encodeToJsonElement(emptySnapshot(20).copy(items = listOf(itemRead("obsolete")))).jsonObject
                }
                return buildJsonObject { put("items", JsonArray(emptyList())) }
            }
        }
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ api }, { if (revision.get() == 0L) "old" else "replacement" }, sessionRevision = revision::get), { now })
        data.sync.fetchListScopes = {
            secondStaged.complete(Unit)
            try { releaseSecond.await() } finally { secondEnded.complete(Unit) }
            emptyList()
        }
        try {
            cache.install(emptySnapshot(0))
            cache.inSession { supervisorScope {
                val first = async(start = CoroutineStart.UNDISPATCHED) { runCatching { data.sync.refresh() } }
                firstStarted.await()
                val second = async(start = CoroutineStart.UNDISPATCHED) { runCatching { data.sync.refresh() } }
                failFirst.complete(Unit); assertIs<ApiProblem>(first.await().exceptionOrNull()); secondStaged.await()
                if (replace) {
                    revision.incrementAndGet()
                    data.sync.refreshSessionChanged(0, cache.storageRevision.value, revision.get())
                    assertNotNull(withTimeoutOrNull(1_000) { secondEnded.await(); true }, "Promoted cohort must still be registered for cancellation")
                    releaseSecond.complete(Unit)
                    assertIs<CancellationException>(second.await().exceptionOrNull())
                    assertEquals(0L, cache.sync.value?.cursor)
                    assertTrue(cache.view.value.items.isEmpty())
                } else {
                    val barrier = async(start = CoroutineStart.UNDISPATCHED) { data.sync.awaitAdmittedRefreshes() }
                    assertFalse(barrier.isCompleted, "Startup must await the promoted pending cohort after its predecessor failed")
                    releaseSecond.complete(Unit); second.await().getOrThrow(); barrier.await()
                    assertEquals(20L, cache.sync.value?.cursor)
                }
            } }
        } finally { failFirst.complete(Unit); releaseSecond.complete(Unit); data.close() }
    }
}
