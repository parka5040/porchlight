package app.porchlight.shared.data

import app.porchlight.shared.FakeSessionBus
import app.porchlight.shared.live.DesktopLive
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAFix1LifecycleTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun trust() = DeviceTrust("device-a", "leaf", "root", "session", "2027-01-01T00:00:00Z",
        listOf("https://example.invalid"), "2027-10-01T00:00:00Z", "member-a")
    private class Keys(var value: DeviceTrust?, var lost: Boolean = false) : DeviceKeyStore {
        override suspend fun load(): DeviceTrust? { if (lost) throw LocalTrustUnavailable(); return value }
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun clear() { value = null; lost = false }
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private class Server : ApiTransport, TrustTransportFactory {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = when (path) {
            "/v1/me" -> buildJsonObject { put("member_id", "member-a") }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
            else -> buildJsonObject {}
        }
    }

    @Test fun freshDesktopReachesEnrollmentWithoutRestart() = lifecycle("empty")
    @Test fun unrecoverableDesktopTrustCanClearAndReadmitWithoutRestart() = lifecycle("lost")
    @Test fun coldPendingLogoutClearsLiveInventoriesBeforeReenrollment() = lifecycle("logout")

    private fun lifecycle(kind: String) = runBlocking {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-ownerless-")
        val file = dir.resolve("cache.db").toFile()
        // Persisted household rows must not be exposed just to unblock live inventory initialization.
        desktopReadCache(file) { now }.also { old ->
            old.initialize(trust().cacheOwner())
            old.install(emptySnapshot(41).copy(items = listOf(itemRead("private"))))
            old.close()
        }
        val cache = desktopReadCache(file) { now }
        val keys = Keys(if (kind == "empty") null else trust().withLogoutPending(kind == "logout"), kind == "lost")
        val controller = TrustController(keys, Server(), "desktop", { now }, false)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        var live: DesktopLive? = null
        var clears = 0
        try {
            // DesktopAppServices installs the real binding, starts DesktopLive, then restores trust.
            data.bindTrust(controller, scope, onIdentityEnded = { live!!.clearIdentity(); clears++ },
                streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
            live = DesktopLive(data, dir, scope, { now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
                sessionFactory = { FakeSessionBus() }, sleepBusFactory = { error("No sleep bus") })
            val reached = withTimeoutOrNull(5_000) {
                controller.restore()
                if (kind == "lost") {
                    assertIs<TrustState.LocalTrustLost>(controller.state.value)
                    controller.startReEnrollment()
                } else if (kind == "logout") {
                    assertEquals(TrustState.Login, controller.state.value)
                    assertFalse(keys.value!!.logoutPending, "Cold logout must finish mandatory live cleanup")
                    controller.startReEnrollment()
                }
                true
            }
            assertEquals(true, reached, "Ownerless identity cleanup must reach Enrollment without deadlocking live initialization")
            assertEquals(TrustState.Enrollment, controller.state.value)
            assertTrue(clears > 0, "Do not skip mandatory desktop live cleanup")
            assertNotNull(withTimeoutOrNull(2_000) { cache.awaitInitialized(); true }, "Cleared ownerless cache must be initialized")
            assertTrue(cache.view.value.items.isEmpty()); assertNull(cache.sync.value)
            assertIs<CacheOwnerUnavailable>(runCatching { cache.inSession { error("Ownerless read admitted") } }.exceptionOrNull())
            // A subsequent accepted trust record can admit household data in this same process.
            keys.value = trust()
            withTimeout(5_000) { controller.restore() }
            assertEquals(TrustState.Ready, controller.state.value)
            cache.install(emptySnapshot(42).copy(items = listOf(itemRead("admitted"))))
            assertEquals("admitted", cache.view.value.items.single().id)
        } finally { live?.close(); scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
