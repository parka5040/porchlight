package app.porchlight.shared.data

import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.io.IOException
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataCacheRecoveryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun nonSqliteRecoversOnlyDisposableCache() = recovery("physical")
    @Test fun malformedPayloadRecoversOnlyDisposableCache() = recovery("payload")
    private fun recovery(damage: String) = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-recovery-").toFile()
        try {
            run {
                val file = root.resolve("$damage.db")
                val sentinel = root.resolve("alarm-high-water").apply { writeText("71") }
                val credentials = root.resolve("trust-metadata").apply { writeText("opaque fixture") }
                if (damage == "physical") file.writeText("not a sqlite database")
                else openDesktopCache(file).let { driver ->
                    PorchlightDb(driver).cacheQueries.putItem("i", "i", null, "{broken")
                    driver.close()
                }
                val cache = desktopReadCache(file) { now }
                val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                try {
                    cache.initialize()
                    assertTrue(cache.view.value.visibilityRevision > 0, "recovery invalidates retained UI copies")
                    assertEquals(CacheView(), cache.view.value.copy(visibilityRevision = 0), "$damage must publish empty")
                    assertLiveBinding(cache, scope)
                    assertEquals("71", sentinel.readText()); assertEquals("opaque fixture", credentials.readText())
                } finally { scope.cancel(); cache.close() }
            }
        } finally { root.deleteRecursively() }
    }

    @Test fun bindingRetriesTransientOpenFailureAndStillCollectsLiveEvents() = runBlocking {
        val cache = ReadCache({
            if (!opened) { opened = true; throw IOException("temporary storage failure") }
            app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver(app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver.IN_MEMORY).also { PorchlightDb.Schema.create(it) }
        }, { now })
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineExceptionHandler { _, _ -> })
        try {
            assertLiveBinding(cache, scope)
        } finally { scope.cancel(); cache.close() }
    }
    @Test fun desktopAlarmStartupRetriesStorageAndClearsItsNotice() = runBlocking {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-alarm-recovery-")
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        var attempts = 0
        val cache = ReadCache({
            if (attempts++ == 0) throw IOException("temporary storage failure")
            app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver(app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver.IN_MEMORY).also { PorchlightDb.Schema.create(it) }
        }, { now })
        val data = HouseholdData(cache, GeneratedCall({ error("Offline") }, { null }), { now })
        val notice = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
        val runtime = app.porchlight.shared.live.DesktopLive(data, root, scope, { now }, { _, _ -> }, {}, notice,
            sessionFactory = { app.porchlight.shared.FakeSessionBus() }, sleepBusFactory = { error("No sleep bus") })
        try {
            withTimeout(5_000) {
                notice.first { it == CACHE_STORAGE_NOTICE }
                data.localSnooze!!("unused") // waits for the real alarm initializer
                cache.storageNotice.first { it == null }
            }
            assertEquals(2, attempts)
        } finally { runtime.close(); scope.cancel(); cache.close(); root.toFile().deleteRecursively() }
    }

    @Test fun recoveryIsBoundedAndNeverDiscardsForTransientStorageErrors() = runTest {
        for (corrupt in listOf(false, true)) {
            var closed = 0
            var discarded = 0
            val cache = ReadCache({
                if (!corrupt) throw IOException("temporary storage failure")
                val driver = app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver(app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver.IN_MEMORY)
                PorchlightDb.Schema.create(driver)
                PorchlightDb(driver).cacheQueries.putItem("i", "i", null, "{broken")
                object : app.cash.sqldelight.db.SqlDriver by driver {
                    override fun close() { driver.close(); closed++ }
                }
            }, { now }, object : CacheRecovery {
                override fun isCorruption(error: Exception) = false
                override fun discard() { discarded++ }
            })
            try {
                repeat(2) { assertFails { cache.initialize() } }
                assertEquals(if (corrupt) 1 else 0, discarded, "one discard per cache lifetime, none for I/O")
                assertEquals(if (corrupt) 3 else 0, closed, "failed handles close before discard/retry")
                assertEquals(CacheView(), cache.view.value.copy(visibilityRevision = 0))
            } finally { cache.close() }
        }
    }
    private var opened = false

    private suspend fun assertLiveBinding(cache: ReadCache, scope: CoroutineScope) {
        val transport = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/snapshot" -> WireJson.encodeToJsonElement(emptySnapshot(7).copy(items = listOf(itemRead("restored")))).jsonObject
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> buildJsonObject {}
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) {
                assertEquals(7, after)
                receive(StreamFrame.Tickle(9, "item.changed", "item", "restored", true))
                awaitCancellation()
            }
        }
        val keys = object : DeviceKeyStore {
            override suspend fun load() = DeviceTrust("self", "leaf", "root", "session", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z")
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) = Unit
            override suspend fun clear() = Unit
            override suspend fun csr() = "CSR"
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val controller = TrustController(keys, transport, "desktop", { now }, false)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        data.bindTrust(controller, scope)
        controller.restore()
        val synced = withContext(Dispatchers.Default) { withTimeoutOrNull(5_000) { cache.sync.first { it?.cursor == 9L } } }
        assertNotNull(synced, "initialization must resume, resnapshot and collect live events")
        assertTrue(cache.view.value.items.isEmpty(), "live eviction after repaired snapshot")
    }
}
