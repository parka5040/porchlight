package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBAndroidLiveTest {
    @Test fun collectorCannotRecreateAfterLogout() = interleave(false, false)
    @Test fun permissionOrWorkerRecoveryCannotRecreateAfterMemberSwitch() = interleave(true, true)
    @Test fun rebuildingKeepsAlarmsButEndedIdentityWithdrawsThem() = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { fixture.now }
        var alarms = 0
        var delivered = 0
        var rescheduled = 0
        val runtime = AndroidLiveReconciler(fixture.controller, { cache.view.value },
            { alarms = 1 }, { delivered = 1 }, { rescheduled = alarms }, { alarms = 0; delivered = 0 })
        try {
            fixture.controller.restore(); cache.install(emptySnapshot(1))
            runtime.reconcile()
            cache.clear() // disposable cache rebuilding, not an identity-end signal
            runtime.reconcile(recover = true)
            assertEquals(1, rescheduled)
            assertEquals(1, delivered)
            fixture.controller.identityEnded = { cache.clear(); runtime.clearIdentity() }
            fixture.controller.logout(); runtime.reconcile(recover = true)
            assertEquals(0, rescheduled)
            assertEquals(0, delivered)
        } finally { fixture.controller.close(); cache.close(); runCurrent() }
    }
    @Test fun rereadsAuthoritativeViewAfterReconciliationPause() = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        var view = CacheView(sync = SyncState(1, fixture.now))
        val paused = CompletableDeferred<Unit>()
        val resume = CompletableDeferred<Unit>()
        var observedCursor: Long? = null
        val runtime = AndroidLiveReconciler(fixture.controller, { view }, {}, { observedCursor = it.sync?.cursor }, {}, {},
            { paused.complete(Unit); resume.await() })
        try {
            fixture.controller.restore()
            val job = launch { runtime.reconcile() }
            paused.await(); view = CacheView(sync = SyncState(2, fixture.now)); resume.complete(Unit); job.join()
            assertEquals(2, observedCursor, "A same-epoch eviction/update also invalidates the captured projection")
        } finally { resume.complete(Unit); fixture.controller.close(); runCurrent() }
    }
    private fun interleave(switch: Boolean, recover: Boolean) = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        val controller = fixture.controller
        val cache = memoryCache { fixture.now }
        val paused = CompletableDeferred<Unit>()
        val resume = CompletableDeferred<Unit>()
        var alarmCount = 0
        var deliveryCount = 0
        var reschedules = 0
        val runtime = AndroidLiveReconciler(controller, { cache.view.value },
            { alarmCount++ }, { deliveryCount++ }, { reschedules++ },
            { alarmCount = 0; deliveryCount = 0; reschedules = 0 },
            { paused.complete(Unit); resume.await() })
        try {
            controller.restore()
            cache.sessionEpoch = { controller.sessionEpoch.value }
            cache.sessionCommitMutex = controller.sessionCommitMutex
            cache.install(emptySnapshot(1))
            controller.identityEnded = { cache.clear(); runtime.clearIdentity() }
            val job = launch { runtime.reconcile(recover) }
            paused.await()
            if (switch) fixture.switchMember() else controller.logout()
            resume.complete(Unit); job.join()
            assertEquals(0, alarmCount, "Old reconciliation recreated alarms after identity end")
            assertEquals(0, deliveryCount, "Old reconciliation recreated delivery after identity end")
            assertEquals(0, reschedules, "Old recovery rescheduled ended-identity inventory")
        } finally { resume.complete(Unit); controller.close(); cache.close(); runCurrent() }
    }
}
