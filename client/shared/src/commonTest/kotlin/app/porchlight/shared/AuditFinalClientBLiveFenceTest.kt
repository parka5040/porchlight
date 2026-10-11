package app.porchlight.shared

import app.porchlight.shared.live.LiveIdentityFence
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBLiveFenceTest {
    @Test fun epochAdvancesWhilePreparedOsWorkAwaitsRuntimeCleanup() = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        val controller = fixture.controller
        val fence = LiveIdentityFence(controller)
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        var posted = false
        try {
            controller.restore()
            controller.identityEnded = { fence.clear { posted = false } }
            val work = launch { fence.run { entered.complete(Unit); release.await(); posted = true } }
            entered.await()
            val logout = launch { controller.logout() }; runCurrent()
            assertFalse(logout.isCompleted)
            assertEquals(1, controller.sessionEpoch.value, "Native work must never block epoch invalidation")
            release.complete(Unit); work.join(); logout.join()
            assertFalse(posted)
        } finally { release.complete(Unit); controller.close(); runCurrent() }
    }
    @Test fun newEpochCannotUseOldViewBeforeCleanupCompletes() = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        val controller = fixture.controller
        val fence = LiveIdentityFence(controller)
        val clearing = CompletableDeferred<Unit>()
        val finish = CompletableDeferred<Unit>()
        try {
            controller.restore()
            controller.identityEnded = { clearing.complete(Unit); finish.await(); fence.clear {} }
            val logout = launch { controller.logout() }
            clearing.await()
            assertEquals(1, controller.sessionEpoch.value)
            assertFalse(fence.run { fail("New epoch cannot admit still-visible old cache content") })
            finish.complete(Unit); logout.join()
            assertFalse(fence.run(0) { fail("Old in-flight work cannot be admitted after cleanup") })
            assertTrue(fence.run {})
        } finally { finish.complete(Unit); controller.close(); runCurrent() }
    }
}
