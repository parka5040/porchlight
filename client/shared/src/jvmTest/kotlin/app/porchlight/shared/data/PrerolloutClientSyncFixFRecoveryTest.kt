package app.porchlight.shared.data

import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncFixFRecoveryTest {
    private val f = SyncBundleFixtures

    @Test fun recoveryPreservesNewDraftAndQuarantinesExistingEditUntilAuthorisedRead() = runTest {
        val cache = memoryCache { f.now }
        val session = ScreenSession(StandardTestDispatcher(testScheduler)).also { it.bindCache(cache) }
        try {
            cache.install(f.snapshot)
            val id = f.snapshot.items.single().id
            val newDraft = session.editor("new", cache.view.value, true)
            newDraft.current(cache.view.value)!!.store(newDraft.current(cache.view.value)!!.draft.copy(title = "Locally authored"))
            val edit = session.editor(id, cache.view.value, false)
            edit.current(cache.view.value)!!.store(edit.current(cache.view.value)!!.draft.copy(title = "Unsaved edit"))
            val writer = session.writes("item:$id")
            val keys = mutableListOf<String>()
            var step = 0
            writer.submit { key ->
                checkNotNull(edit.current(cache.view.value)) { "Editor visibility changed" }
                while (step < 2) {
                    keys += "$key-$step"
                    if (keys.size == 2) error("Second step committed but reply lost")
                    step++
                }
                WriteResult.Success(JsonNull)
            }
            val key = writer.state.value.retryKey
            cache.clearLocalCache()
            assertEquals("Locally authored", newDraft.current(cache.view.value)?.draft?.title, "Same-identity recovery must preserve new drafts")
            assertNull(edit.current(cache.view.value), "Server-dependent input is hidden until fresh authorisation")
            assertSame(edit, session.editor(id, cache.view.value, false), "Quarantined editors must not be replaced")
            cache.install(f.snapshot)
            assertEquals("Unsaved edit", edit.current(cache.view.value)?.draft?.title)
            writer.retry()
            assertEquals(listOf("$key-0", "$key-1", "$key-1"), keys)
            assertTrue(writer.state.value.saved)
            cache.clearLocalCache()
            cache.install(f.snapshot.copy(items = emptyList(), occurrences = emptyList()))
            assertNull(edit.current(cache.view.value), "Omitted resources never regain their old representations")
        } finally { session.close(); cache.close() }
    }

    @Test fun replacementIdentityInterruptsRealSnapshotRetry() = replacement("identity")
    @Test fun replacementTokenInterruptsRealSnapshotRetry() = replacement("token")
    @Test fun replacementStorageInterruptsRealSnapshotRetry() = replacement("storage")

    @Test fun sessionChangeCancelsTargetedSnapshotWithoutAReplacementWaiter() = replacement("token", notifyOnly = true)

    private fun replacement(change: String, notifyOnly: Boolean = false) = runTest {
        val cache = memoryCache { f.now }
        var epoch = 0L; var revision = 0L; var requests = 0
        cache.sessionEpoch = { epoch }
        val sleeping = CompletableDeferred<Unit>()
        val wire = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when {
                path.startsWith("/v1/snapshot") -> {
                    if (++requests == 1) throw ApiProblem("unavailable", 503, "Retry", retryAfterMillis = 300_000)
                    WireJson.encodeToJsonElement(f.snapshot).jsonObject
                }
                path.startsWith("/v1/board") || path.startsWith("/v1/agenda") || path.startsWith("/v1/lists") -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> error("Unexpected request")
            }
        }
        val call = GeneratedCall({ wire }, { "session-$revision" }, epoch = { epoch }, sessionRevision = { revision })
        val data = HouseholdData(cache, call, { f.now }, syncDispatcher = Dispatchers.Default)
        data.itemSyncCapability = { notifyOnly }
        data.targetedSyncVerified = true
        data.sync.waitForReadRetry = { sleeping.complete(Unit); delay(it) }
        try {
            cache.initialize()
            val old = async { runCatching { data.sync.refresh() } }
            sleeping.await()
            when (change) { "identity" -> epoch++; "token" -> revision++; else -> cache.clearLocalCache() }
            if (notifyOnly) {
                data.sync.refreshSessionChanged(epoch, cache.storageRevision.value, revision)
                withContext(Dispatchers.Default) { withTimeout(2_000) { old.await() } }
                assertEquals(1, requests, "Changing admission cancels a sleeper even without replacement demand")
            }
            val replacement = async(Dispatchers.Default) { data.sync.refresh() }
            withContext(Dispatchers.Default) { withTimeout(2_000) { replacement.await(); old.await() } }
            assertTrue(old.await().exceptionOrNull() is CancellationException, "Obsolete full-refresh work must be cancelled")
            assertEquals(2, requests, "The sleeping request must never retry under replacement admission")
        } finally { data.close() }
    }
}
