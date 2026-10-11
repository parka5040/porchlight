package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import java.util.concurrent.CyclicBarrier
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient3aPinTest {
    private class Keys : DeviceKeyStore {
        var value = DeviceTrust("self", "leaf", "root", "session", "2027-01-01T00:00:00Z", listOf("https://lan:8443"), "2027-10-01T00:00:00Z")
        var loading: (suspend () -> Unit)? = null
        override suspend fun load(): DeviceTrust { loading?.invoke(); return value }
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun clear() {}
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = value.certificateExpiresAt
    }
    private class Factory : TrustTransportFactory, ApiTransport {
        var request: (suspend () -> Unit)? = null
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
            request?.invoke()
            return WireJson.parseToJsonElement("""{"session":{"token":"session","expires_at":"2027-01-01T00:00:00Z"}}""").jsonObject
        }
    }
    private fun controller(keys: Keys, factory: Factory) = TrustController(keys, factory, "android", { Instant.parse("2026-10-07T12:00:00Z") }, true)

    @Test fun restoreRelocksBeforeSuspendingOnStorage() = runBlocking {
        val keys = Keys(); val controller = controller(keys, Factory())
        val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        var restoring: Job? = null
        try {
            controller.restore(); controller.unlockPin()
            keys.loading = { entered.complete(Unit); release.await() }
            restoring = launch { controller.restore() }
            entered.await()
            assertEquals(TrustState.PinRequired, controller.state.value, "Restoring PIN requirement must hide ready content before I/O")
        } finally { release.complete(Unit); restoring?.join(); controller.close() }
    }
    @Test fun relockDoesNotWaitForAnInFlightLoginAndItsReplyCannotUnlock() = runBlocking {
        val factory = Factory(); val controller = controller(Keys(), factory)
        val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        var login: Job? = null
        try {
            controller.restore(); controller.unlockPin()
            factory.request = { entered.complete(Unit); release.await() }
            login = launch { controller.login("fixture") }
            entered.await()
            controller.lockPin()
            assertEquals(TrustState.PinRequired, controller.state.value)
            release.complete(Unit); login.join()
            assertEquals(TrustState.PinRequired, controller.state.value)
        } finally { release.complete(Unit); login?.join(); controller.close() }
    }
    @Test fun concurrentLoginPublicationAndRelockKeepThePinGate() = runBlocking {
        val controller = controller(Keys(), Factory())
        val executor = Executors.newFixedThreadPool(2)
        val start = CyclicBarrier(3); val finish = CyclicBarrier(3)
        val rounds = 10_000
        try {
            controller.restore()
            val login = executor.submit { repeat(rounds) { start.await(); runBlocking { controller.login("fixture") }; finish.await() } }
            val lock = executor.submit { repeat(rounds) { start.await(); controller.lockPin(); finish.await() } }
            var exposed = 0
            repeat(rounds) {
                controller.unlockPin(); start.await(); finish.await()
                if (controller.state.value != TrustState.PinRequired) exposed++
            }
            login.get(10, TimeUnit.SECONDS); lock.get(10, TimeUnit.SECONDS)
            assertEquals(0, exposed, "A concurrent signed-in publication overwrote the PIN lock")
        } finally { executor.shutdownNow(); assertTrue(executor.awaitTermination(10, TimeUnit.SECONDS)); controller.close() }
    }
}
