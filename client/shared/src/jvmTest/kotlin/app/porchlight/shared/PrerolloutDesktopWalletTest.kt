package app.porchlight.shared

import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class PrerolloutDesktopWalletTest {
    private val factory = object : TrustTransportFactory {
        override fun enrollment(uri: EnrollUri): ApiTransport = error("Not needed")
        override suspend fun authenticated(trust: DeviceTrust): ApiTransport = error("Not needed")
    }
    @Test fun transientWalletAbsenceRecoversAutomatically() = runTest {
        val f = AuditFinalClientCFixture()
        var available = false; var loads = 0
        val keys = object : DeviceKeyStore by f.keys {
            override suspend fun load(): DeviceTrust? { loads++; if (!available) throw WalletUnavailable(); return null }
        }
        val controller = TrustController(keys, factory, "desktop", { f.now }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); assertEquals(TrustState.WalletLocked, controller.state.value)
            available = true
            advanceTimeBy(60_000); runCurrent()
            assertEquals(TrustState.Enrollment, controller.state.value, "Transient wallet availability must be retried")
            assertEquals(2, loads)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun dismissedUnlockNeverRepromptsUntilExplicitRetry() = runTest {
        val f = AuditFinalClientCFixture(); var loads = 0
        val keys = object : DeviceKeyStore by f.keys {
            override suspend fun load(): DeviceTrust? { loads++; throw WalletUnlockDismissed() }
        }
        val controller = TrustController(keys, factory, "desktop", { f.now }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); advanceTimeBy(3_600_000); runCurrent()
            assertEquals(1, loads)
            assertEquals(TrustState.WalletLocked, controller.state.value)
            controller.restore(); assertEquals(2, loads)
        } finally { controller.close(); runCurrent() }
    }
}
