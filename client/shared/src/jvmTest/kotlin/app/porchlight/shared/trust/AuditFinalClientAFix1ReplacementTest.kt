package app.porchlight.shared.trust

import app.porchlight.shared.data.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAFix1ReplacementTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Keys : DeviceKeyStore {
        var value = DeviceTrust("device-a", "leaf", "root", "session", "2027-01-01T00:00:00Z",
            listOf("https://example.invalid"), "2027-10-01T00:00:00Z", "member-a")
        var saving: suspend () -> Unit = {}
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saving(); value = trust }
        override suspend fun clear() = Unit
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = value.certificateExpiresAt
    }
    private class Server : ApiTransport, TrustTransportFactory {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = when (path) {
            "/v1/me" -> buildJsonObject { put("member_id", "member-b") }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
            else -> buildJsonObject {}
        }
    }
    private class Fixture(val now: Instant) {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-replacement-")
        val keys = Keys()
        val controller = TrustController(keys, Server(), "android", { now }, true)
        var clearing: suspend () -> Unit = {}
        val cache = ReadCache({ openDesktopCache(dir.resolve("cache.db").toFile()) }, { now }, null,
            object : AttachmentStore by MemoryAttachmentStore() { override suspend fun clear() { clearing() } })
        val call = GeneratedCall.trusted(controller)
        // Install production admission hooks, but drive endpoint refresh explicitly instead of starting collectors.
        val scope = CoroutineScope(SupervisorJob() + StandardTestDispatcher())
        init { HouseholdData(cache, call, { now }).bindTrust(controller, scope) }
        suspend fun start() {
            controller.restore(); controller.unlockPin()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("private"))))
        }
        fun close() { scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }

    @Test fun relockDuringReplacementSaveCannotPublishReady() = pinRace(save = true, unlock = false)
    @Test fun relockDuringReplacementCleanupCannotPublishReady() = pinRace(save = false, unlock = false)
    @Test fun unlockDuringReplacementUsesCurrentPinState() = pinRace(save = true, unlock = true)
    private fun pinRace(save: Boolean, unlock: Boolean) = runBlocking {
        val f = Fixture(now); val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        var refresh: Deferred<Unit>? = null
        try {
            f.start()
            if (unlock) f.controller.lockPin()
            val hold: suspend () -> Unit = { entered.complete(Unit); release.await() }
            if (save) f.keys.saving = hold else f.clearing = hold
            refresh = async { f.controller.refreshEndpointsAndRenew() }
            withTimeout(5_000) { entered.await() }
            assertEquals(TrustState.Loading, f.controller.state.value, "Replacement is an admission gate")
            assertFailsWith<HouseholdLocked> { f.call.ensureReady() }
            if (unlock) f.controller.unlockPin() else f.controller.lockPin()
            release.complete(Unit); withTimeout(5_000) { refresh.await() }
            assertEquals(if (unlock) TrustState.Ready else TrustState.PinRequired, f.controller.state.value,
                "Replacement admission must use the current PIN state, not a captured gate")
            assertEquals("member-b", f.keys.value.memberId)
            assertTrue(f.cache.view.value.items.isEmpty(), "Previous household content must be purged")
            if (!unlock) assertFailsWith<HouseholdLocked> { f.call.ensureReady() }
        } finally { release.complete(Unit); refresh?.cancelAndJoin(); f.close() }
    }

    @Test fun failedReplacementSaveHasAnActionableWorkingRetry() = failedAdmission(save = true)
    @Test fun failedReplacementCleanupHasAnActionableWorkingRetry() = failedAdmission(save = false)
    private fun failedAdmission(save: Boolean) = runBlocking {
        val f = Fixture(now)
        try {
            f.start()
            if (save) f.keys.saving = { error("Storage unavailable") } else f.clearing = { error("Cleanup unavailable") }
            assertTrue(runCatching { f.controller.refreshEndpointsAndRenew() }.isFailure)
            assertIs<TrustState.LocalTrustLost>(f.controller.state.value, "Failed replacement must offer recovery, never strand Loading")
            assertFailsWith<HouseholdLocked> { f.call.ensureReady() }
            f.keys.saving = {}; f.clearing = {}
            withTimeout(5_000) { f.controller.restore() }
            assertEquals(TrustState.PinRequired, f.controller.state.value)
            f.controller.unlockPin()
            withTimeout(5_000) { f.controller.refreshEndpointsAndRenew() }
            assertEquals(TrustState.Ready, f.controller.state.value)
            assertEquals("member-b", f.keys.value.memberId)
            assertTrue(f.cache.view.value.items.isEmpty())
        } finally { f.close() }
    }
}
