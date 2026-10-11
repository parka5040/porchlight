package app.porchlight.shared.trust

import kotlin.test.*

class AuditBlClient3bWalletTest {
    @Test fun probeCleanupIsIdempotentAndDoesNotDeleteOtherAccounts() {
        val bus = FakeSecretService()
        val probe = DirectSecretService("app.porchlight.test", "probe", bus, 100, {})
        val other = DirectSecretService("app.porchlight.test", "other", bus, 100, {})
        other.write("other fixture")
        assertFailsWith<IllegalStateException> {
            try { probe.write("probe fixture"); error("Simulated assertion failure") }
            finally { probe.deleteProbe() }
        }
        probe.deleteProbe()
        assertNull(probe.read())
        assertEquals("other fixture", other.read())
        assertFailsWith<IllegalArgumentException> {
            DirectSecretService(DesktopSecretWallet.SERVICE, DesktopSecretWallet.ACCOUNT, bus, 100, {}).deleteProbe()
        }
    }
}
