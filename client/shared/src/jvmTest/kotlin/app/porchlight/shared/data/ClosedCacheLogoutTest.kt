package app.porchlight.shared.data

import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.ApiTransport
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

/** A closed cache rejects cleanup; logout must retain a durable obligation instead of losing it. */
class ClosedCacheLogoutTest {
    @Test fun closedCacheClearIsNotALogoutBackstopAndRowsRemainOnDisk() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val dir = Files.createTempDirectory(Path.of(checkNotNull(System.getProperty("porchlight.test.dir"))), "closed-logout-")
        val file = dir.resolve("read-cache.db").toFile()
        val cache = ReadCache(openDesktopCache(file), { now })
        val keys = object : DeviceKeyStore {
            var value = DeviceTrust("device", "leaf", "root", "session", "2027-10-01T00:00:00Z",
                listOf("https://lan:8443"), "2027-10-01T00:00:00Z")
            override suspend fun load() = value
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
            override suspend fun clear() { error("not called") }
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?,
                idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = transport
            override suspend fun authenticated(trust: DeviceTrust) = transport
        }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        try {
            cache.install(emptySnapshot(42).copy(items = listOf(itemRead("logged-out-row"))))
            controller.restore()
            var attempted = false
            // Same durable-clear callback installed by HouseholdData.bindTrust, with no binding scheduler race.
            controller.identityEnded = { attempted = true; cache.clear() }
            cache.close()
            assertFailsWith<CancellationException> { controller.logout() }
            assertTrue(keys.value.logoutPending)
            assertTrue(attempted); assertEquals(TrustState.Login, controller.state.value)
            assertNull(keys.value.session)
            // The binding's epoch-change clear calls this same clear, and fails on the same closed cache.
            assertEquals("Read cache closed", assertFailsWith<CancellationException> { cache.clear() }.message)
            val reopened = ReadCache(openDesktopCache(file), { now })
            try {
                reopened.initialize()
                assertEquals("logged-out-row", reopened.view.value.items.single().id)
                assertEquals(42L, reopened.sync.value?.cursor)
                reopened.clear() // only an open cache can finally remove the rows
            } finally { reopened.close() }
            val verified = ReadCache(openDesktopCache(file), { now })
            try { verified.initialize(); assertTrue(verified.view.value.items.isEmpty()) } finally { verified.close() }
        } finally { controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
