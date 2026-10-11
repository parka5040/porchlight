package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCTerminalTest {
    @Test fun streamRevocationPublishesTerminalGateBeforeCancelledCleanup() = revoked(false)
    @Test fun streamTlsRevocationPublishesTerminalGateBeforeCancelledCleanup() = revoked(true)
    private fun revoked(tls: Boolean) = runTest {
        val f = AuditFinalClientCFixture()
        val controller = f.controller(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "terminal-")
        val stream = CompletableDeferred<Unit>(); val revoke = CompletableDeferred<Unit>()
        val cleanup = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        f.onStream = { stream.complete(Unit); revoke.await(); if (tls) throw DeviceCertificateRevoked() else throw ApiProblem("device-revoked", 401, "") }
        val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
            sessionFactory = { FakeSessionBus() }, sleepBusFactory = { NoSleepBus })
        data.bindTrust(controller, backgroundScope, onIdentityEnded = { epoch ->
            cleanup.complete(Unit); release.await(); live.clearIdentity(epoch)
        })
        try {
            controller.restore(); stream.await(); revoke.complete(Unit); cleanup.await(); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value, "Stream must publish ReEnroll even when its epoch cancels cleanup")
            assertNull(controller.liveAdmission.value)
            release.complete(Unit); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value)
            assertEquals(controller.sessionEpoch.value, controller.liveAdmission.value)
            assertNull(cache.sync.value)
            assertFailsWith<CacheOwnerUnavailable> { cache.inSession { error("Ownerless cache admitted") } }
        } finally { release.complete(Unit); live.close(); controller.close(); cache.close(); runCurrent(); dir.toFile().deleteRecursively() }
    }
}
