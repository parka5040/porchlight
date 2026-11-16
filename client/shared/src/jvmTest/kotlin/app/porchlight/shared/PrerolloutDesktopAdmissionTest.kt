package app.porchlight.shared

import kotlinx.coroutines.*
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.*

class PrerolloutDesktopAdmissionTest {
    @Test fun readyDoesNotWaitForOptionalBusRegistration() = runBlocking {
        val release = CountDownLatch(1)
        val entered = CountDownLatch(1)
        val bus = FakeSessionBus()
        val runtime = AuditFinalClientCNotifyRuntime(bus, sessionFactory = {
            entered.countDown(); check(release.await(10, TimeUnit.SECONDS)); bus
        })
        val start = async(Dispatchers.Default) { runtime.start() }
        try {
            assertNotNull(withTimeoutOrNull(2_000) { start.await(); true }, "Trusted content admission must not wait for D-Bus")
            assertTrue(withContext(Dispatchers.IO) { entered.await(2, TimeUnit.SECONDS) }, "Optional registration starts after Ready")
        } finally {
            release.countDown()
            start.await()
            runtime.close()
        }
    }
}
