package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCInitializationTest {
    @Test fun storageRepairRecoversOwnerlessCleanupWithoutReconstructingRuntime() = recovery(true)
    @Test fun storageRepairRecoversKnownIdentityAdmissionWithoutReconstructingRuntime() = recovery(false)
    private fun recovery(empty: Boolean) = runBlocking {
        val f = AuditFinalClientCFixture(); if (empty) f.saved = null
        val controller = f.controller(Dispatchers.Default)
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "native-init-")
        val cache = desktopReadCache(dir.resolve("cache.db").toFile()) { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
        val blocked = Files.createDirectory(dir.resolve("os-delivery.json.new"))
        if (empty) Files.writeString(dir.resolve("os-delivery.json"), "damaged inventory")
        val repaired = java.util.concurrent.atomic.AtomicBoolean()
        val notice = MutableStateFlow<String?>(null)
        val failures = java.util.concurrent.CopyOnWriteArrayList<String>()
        val attempts = java.util.concurrent.atomic.AtomicInteger()
        val lifetime = CoroutineScope(SupervisorJob() + Dispatchers.Default + CoroutineExceptionHandler { _, e -> failures += e.javaClass.simpleName })
        val live = DesktopLive(data, dir, lifetime, { f.now }, { _, _ -> }, {}, notice, controller,
            sessionFactory = { attempts.incrementAndGet(); FakeSessionBus() }, sleepBusFactory = { NoSleepBus },
            alarmLoader = { store -> check(repaired.get()) { "Storage unavailable" }; store.load() })
        data.bindTrust(controller, lifetime, onIdentityEnded = { live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        try {
            controller.restore(); assertNull(controller.liveAdmission.value)
            withTimeout(5_000) { notice.first { it.orEmpty().contains("Retrying inventory recovery") && it.orEmpty().contains("Local alarms unavailable") } }
            repaired.set(true)
            Files.delete(blocked)
            val admitted = withTimeoutOrNull(5_000) { controller.liveAdmission.first { it != null }; true }
            assertEquals(true, admitted, "Repaired native storage must retry initialization, not await a permanently failed deferred: state=${controller.state.value}, attempts=${attempts.get()}, failures=$failures")
            assertEquals(if (empty) TrustState.Enrollment else TrustState.Ready, controller.state.value)
            assertTrue(Files.isRegularFile(dir.resolve("os-delivery.json")))
            assertFalse(notice.value.orEmpty().contains("Retrying inventory recovery"), "Repaired initialization must clear its retry banner")
            assertFalse(notice.value.orEmpty().contains("Local alarms unavailable"), "Successful alarm reload must clear its stale failure banner")
            if (empty) assertTrue(notice.value.orEmpty().contains("Local delivery inventory damaged"), "Recovery must keep the unrelated damage warning")
        } finally { live.close(); lifetime.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
