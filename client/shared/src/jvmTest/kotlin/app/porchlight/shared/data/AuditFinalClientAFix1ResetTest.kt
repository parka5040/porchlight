package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAFix1ResetTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Keys : DeviceKeyStore {
        var value = DeviceTrust("device-a", "leaf", "root", "session", "2027-01-01T00:00:00Z",
            listOf("https://example.invalid"), "2027-10-01T00:00:00Z", "member-a")
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun clear() = Unit
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = value.certificateExpiresAt
    }

    @Test fun boundRecreationRestartsHealthyStreamAndRejectsOldFence() = reset("recreate")
    @Test fun deletionBetweenCursorProbeAndLoadCannotConsumeReset() = reset("between")
    @Test fun acknowledgmentUsesTheSameSnapshotAsCursorProbe() = reset("acknowledge")
    @Test fun sameOwnerAdmissionDetectsCursorLoss() = reset("bind")

    private fun reset(kind: String) = runBlocking {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-reset-snapshot-")
        val file = dir.resolve("cache.db").toFile()
        fun externalDelete() {
            val external = openDesktopCache(file)
            try { PorchlightDb(external).cacheQueries.clearSync() } finally { external.close() }
        }
        val inject = AtomicBoolean(false); val injected = AtomicBoolean(false)
        val cache = ReadCache({
            val delegate = openDesktopCache(file)
            // WAL lets a second SQLite writer commit while a reader's snapshot remains open.
            delegate.executeQuery(null, "PRAGMA journal_mode=WAL", { QueryResult.Value(Unit) }, 0)
            object : SqlDriver by delegate {
                override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> {
                    val result = delegate.executeQuery(identifier, sql, mapper, parameters, binders)
                    if (sql.contains("FROM SyncState", ignoreCase = true) && inject.compareAndSet(true, false)) {
                        externalDelete(); injected.set(true)
                    }
                    return result
                }
            }
        }, { now })
        val heartbeat = CompletableDeferred<Unit>(); val snapshot = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val started = CompletableDeferred<Unit>(); val finish = CompletableDeferred<Unit>()
        val server = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/me" -> buildJsonObject { put("member_id", "member-a") }
                "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
                "/v1/snapshot" -> { snapshot.complete(Unit); release.await(); WireJson.encodeToJsonElement(emptySnapshot(2)).jsonObject }
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> buildJsonObject {}
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) {
                while (currentCoroutineContext().isActive) { receive(StreamFrame.Heartbeat); heartbeat.complete(Unit); delay(25) }
            }
        }
        val keys = Keys(); val controller = TrustController(keys, server, "desktop", { now }, false)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        try {
            cache.initialize(keys.value.cacheOwner()); cache.install(emptySnapshot(1))
            data.bindTrust(controller, scope, resumeFromCursor = true); controller.restore()
            withTimeout(5_000) { heartbeat.await() }
            // Finish the automatic same-owner endpoint save before arming the SQLite injection.
            withTimeout(5_000) { controller.refreshEndpointsAndRenew() }
            val before = cache.storageRevision.value
            val old = async { cache.inSession { fence -> started.complete(Unit); finish.await(); runCatching { cache.install(fence, emptySnapshot(99)) } } }
            withTimeout(5_000) { started.await() }
            when (kind) {
                "recreate" -> {
                    assertTrue(file.delete())
                    for (suffix in listOf("-wal", "-shm")) file.resolveSibling(file.name + suffix).delete()
                    openDesktopCache(file).close()
                    cache.initialize()
                }
                "bind" -> { externalDelete(); cache.initialize(keys.value.cacheOwner()) }
                else -> {
                    inject.set(true)
                    // An acknowledgment may hit SQLITE_BUSY_SNAPSHOT, but cannot consume reset signalling.
                    if (kind == "acknowledge") runCatching { cache.acknowledgeCursor(5) } else cache.initialize()
                    assertTrue(injected.get(), "The writer must delete after the cursor probe and before payload load")
                    cache.initialize()
                }
            }
            assertTrue(cache.storageRevision.value > before, "Every discarded cursor must signal storage loss, including owner reconciliation")
            assertNotNull(withTimeoutOrNull(2_000) { snapshot.await(); true }, "A healthy stream must request a full snapshot after cache loss")
            finish.complete(Unit)
            assertIs<CancellationException>(withTimeout(5_000) { old.await() }.exceptionOrNull(), "Pre-reset fence must be rejected")
            release.complete(Unit)
            withTimeout(5_000) { cache.sync.first { it?.cursor == 2L } }
            Unit
        } finally { finish.complete(Unit); release.complete(Unit); scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
