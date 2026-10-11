package app.porchlight.shared.data

import app.porchlight.shared.AuditFinalClientBFix1Fixture
import app.porchlight.shared.trust.LocalTrustUnavailable
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientAMergeAdmissionTest {
    @Test fun failedOwnerAdmissionCannotOpenLiveDelivery() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        var blocked = true
        val cache = memoryCache { f.now }
        HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now }).bindTrust(f.controller, backgroundScope,
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        val admit = f.controller.identityStarting
        f.controller.identityStarting = { value ->
            check(!blocked) { "Owner admission unavailable" }
            admit(value)
        }
        try {
            f.controller.restore()
            assertIs<TrustState.LocalTrustLost>(f.controller.state.value)
            assertNull(f.controller.liveAdmission.value, "Failed owner admission must not prove live restoration")
            blocked = false
            f.controller.restore()
            assertEquals(TrustState.Ready, f.controller.state.value)
            assertEquals(f.controller.sessionEpoch.value, f.controller.liveAdmission.value)
        } finally { f.controller.close(); cache.close() }
    }

    @Test fun explicitLostTrustRecoveryOpensTheCleanOwnerlessEpoch() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }
        var nativeClears = 0
        HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now }).bindTrust(f.controller, backgroundScope,
            onIdentityEnded = { nativeClears++ }, streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        f.beforeLoad = { throw LocalTrustUnavailable() }
        try {
            f.controller.restore()
            assertIs<TrustState.LocalTrustLost>(f.controller.state.value)
            f.controller.startReEnrollment()
            assertEquals(TrustState.Enrollment, f.controller.state.value)
            assertEquals(1, nativeClears)
            assertEquals(f.controller.sessionEpoch.value, f.controller.liveAdmission.value,
                "Explicit ownerless cleanup must finish restoration without a process restart")
            cache.awaitInitialized()
            assertFailsWith<CacheOwnerUnavailable> { cache.inSession { error("Ownerless read admitted") } }
        } finally { f.controller.close(); cache.close() }
    }

    @Test fun completedCleanupCannotEraseAnAdmittedReplacement() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }
        var nativeClears = 0
        HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now }).bindTrust(f.controller, backgroundScope,
            onIdentityEnded = { nativeClears++ }, streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        try {
            // The binding collector runs only AFTER synchronous replacement admission has completed.
            f.controller.restore()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
            f.switchMember()
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead("replacement"))))
            runCurrent()
            assertEquals(listOf("replacement"), cache.view.value.items.map { it.id })
            assertEquals(2, cache.sync.value?.cursor)
            assertEquals(1, nativeClears, "The completed epoch's collector is a no-op")
        } finally { f.controller.close(); cache.close() }
    }

    @Test fun nativeFailureGatesReplacementAndAutonomousRetryPreservesItsRows() = runTest {
        withContext(Dispatchers.Default) {
            val f = AuditFinalClientBFix1Fixture(Dispatchers.Default)
            val cache = memoryCache { f.now }
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val blocked = MutableStateFlow(false)
            val nativeClears = MutableStateFlow(0)
            HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now }).bindTrust(f.controller, scope,
                onIdentityEnded = {
                    nativeClears.value++
                    check(!blocked.value) { "Native inventory unavailable" }
                }, streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
            try {
                f.controller.restore()
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
                blocked.value = true
                f.switchMember()
                assertIs<TrustState.LocalTrustLost>(f.controller.state.value,
                    "Native failure must gate replacement admission, not just delivery")
                assertNull(f.controller.liveAdmission.value)
                assertTrue(cache.view.value.items.isEmpty())
                blocked.value = false
                // Native-only failures must trigger owner readmission, not just cache/file retry.
                withTimeout(5_000) { f.controller.state.first { it == TrustState.Ready } }
                assertEquals(1L, f.controller.liveAdmission.value)
                cache.install(emptySnapshot(2).copy(items = listOf(itemRead("replacement"))))
                val completed = nativeClears.value
                delay(1_200)
                assertEquals(completed, nativeClears.value, "A late collector cannot repeat completed cleanup")
                assertEquals(listOf("replacement"), cache.view.value.items.map { it.id })
            } finally {
                scope.cancel(); f.controller.close(); cache.close()
            }
        }
    }
}
