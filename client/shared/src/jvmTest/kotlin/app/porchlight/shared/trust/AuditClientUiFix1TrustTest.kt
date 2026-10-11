package app.porchlight.shared.trust

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.data.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.json.*
import java.io.File
import kotlin.test.*
import kotlin.time.Instant

class AuditClientUiFix1TrustTest {
    private val now = Instant.parse("2026-10-11T12:00:00Z")
    private fun trusted(id: String = "first") = DeviceTrust(id, "leaf", "root-$id", "session-$id", "2027-04-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z")
    private class Keys(var durable: String) : DeviceKeyStore {
        var crashAfterNullSession = false
        var rejectSave = false
        override suspend fun load() = WireJson.decodeFromString<DeviceTrust>(durable)
        override suspend fun csr() = "CSR"
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) {
            if (rejectSave) error("Storage unavailable")
            durable = WireJson.encodeToString(trust)
            if (crashAfterNullSession && trust.session == null) throw CancellationException("Process stopped after session removal")
        }
        override suspend fun clear() { }
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private class Factory(val answer: suspend (String) -> JsonObject) : TrustTransportFactory, ApiTransport {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = answer(path)
    }
    @Test fun restartAfterRemoteCommitCompletesLogoutOffline() = crash(false)
    @Test fun restartAfterSessionRemovalCompletesCacheClear() = crash(true)
    private fun crash(afterSession: Boolean) = runBlocking {
        val dir = File("../../var/fix1-trust-${if (afterSession) "session" else "remote"}").apply { mkdirs() }
        val db = File(dir, "cache.db").apply { delete() }
        fun cache(): ReadCache {
            val existed = db.exists()
            val driver = JdbcSqliteDriver("jdbc:sqlite:${db.absolutePath}")
            if (!existed) PorchlightDb.Schema.create(driver)
            return ReadCache(driver) { now }
        }
        val keys = Keys(WireJson.encodeToString(trusted()))
        val first = TrustController(keys, Factory { if (!afterSession) throw CancellationException("Process stopped after remote commit"); buildJsonObject {} }, "desktop", { now }, false)
        val oldCache = cache()
        first.identityEnded = { oldCache.clear() }
        try {
            first.restore()
            oldCache.inSession { oldCache.putItem(it, itemResponse("private")) }
            keys.crashAfterNullSession = afterSession
            assertFailsWith<CancellationException> { first.logout() }
        } finally { first.close(); oldCache.close() }
        val restartedCache = cache()
        val restarted = TrustController(Keys(keys.durable), Factory { error("Restart must clean locally without network") }, "desktop", { now }, false)
        restarted.identityEnded = { restartedCache.clear() }
        try {
            restartedCache.initialize()
            assertTrue(restartedCache.view.value.items.isNotEmpty(), "fixture must retain old cache before recovery")
            restarted.restore()
            assertEquals(TrustState.Login, restarted.state.value, "pending logout must never restore an active session")
            assertNull(restarted.sessionToken())
            assertTrue(restartedCache.view.value.items.isEmpty(), "pending logout must durably clear old cache")
        } finally { restarted.close(); restartedCache.close(); dir.deleteRecursively() }
    }
    @Test fun androidBackgroundBindsCleanupBeforeRestoring() {
        val source = File("src/androidMain/kotlin/app/porchlight/shared/live/AndroidLive.kt").readText()
        for (entry in listOf("NotificationActionWorker", "HouseholdSyncWorker", "HouseholdStreamService")) {
            val start = source.indexOf("val services = AndroidAppServices.get(", source.indexOf("class $entry"))
            val restore = source.indexOf("ensureRestored()", start)
            assertTrue(source.substring(start, restore).contains("services.data"), "$entry must bind durable cleanup before restore")
        }
    }
    @Test fun logoutNeverSendsBeforeItsIntentIsDurable() = runBlocking {
        val keys = Keys(WireJson.encodeToString(trusted()))
        var calls = 0
        val controller = TrustController(keys, Factory { calls++; buildJsonObject {} }, "desktop", { now }, false)
        try {
            controller.restore(); keys.rejectSave = true
            assertFailsWith<IllegalStateException> { controller.logout() }
            assertEquals(0, calls)
        } finally { controller.close() }
    }
    @Test fun cleanupFailureKeepsMarkerUntilSuccessfulRestart() = runBlocking {
        val keys = Keys(WireJson.encodeToString(trusted().withLogoutPending(true)))
        val first = TrustController(keys, Factory { error("Offline") }, "desktop", { now }, false)
        first.identityEnded = { error("Storage unavailable") }
        try {
            first.restore()
            assertNotEquals(TrustState.Ready, first.state.value)
            assertTrue(keys.load().logoutPending)
            assertNull(keys.load().session)
            assertFailsWith<IllegalStateException> { first.startReEnrollment() }
            assertTrue(keys.load().logoutPending, "re-enrollment must not discard pending cleanup")
        } finally { first.close() }
        var cleared = false
        val reloaded = Keys(keys.durable)
        val second = TrustController(reloaded, Factory { error("Offline") }, "desktop", { now }, false)
        second.identityEnded = { cleared = true }
        try {
            second.restore()
            assertTrue(cleared)
            assertEquals(TrustState.Login, second.state.value)
            assertFalse(reloaded.load().logoutPending, "clear marker only after durable cleanup")
        } finally { second.close() }
    }
    @Test fun retryCannotCrossIdentityOrSessionReplacement() = runBlocking {
        for (sameDevice in listOf(false, true)) {
            val keys = Keys(WireJson.encodeToString(trusted()))
            var second = false; var secondCalls = 0
            val firstTransport = Factory { throw Exception("Ambiguous response") }
            val secondTransport = Factory { secondCalls++; buildJsonObject {} }
            val factory = object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri) = secondTransport
                override suspend fun authenticated(trust: DeviceTrust) = if (second) secondTransport else firstTransport
            }
            val controller = TrustController(keys, factory, "desktop", { now }, false)
            try {
                controller.restore()
                controller.runAccountAction("Changed") { controller.changePassword("prior-password", "new-password", "original-key") }!!.join()
                assertTrue(controller.accountRetry.value)
                if (!sameDevice) controller.startReEnrollment()
                keys.durable = WireJson.encodeToString(if (sameDevice) trusted().withSession("replacement", "2027-04-01T00:00:00Z") else trusted("second"))
                second = true
                controller.restore()
                controller.retryAccountAction()?.join()
                assertEquals(0, secondCalls, "old account intent must never reach replacement transport")
                assertFalse(controller.accountRetry.value)
                assertNull(controller.accountResult.value)
            } finally { controller.close() }
        }
    }
}
