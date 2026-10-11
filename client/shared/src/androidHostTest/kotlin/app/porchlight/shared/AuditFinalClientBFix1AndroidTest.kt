package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.coroutines.flow.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBFix1AndroidTest {
    @Test fun permissionAndBootRecoveryAwaitColdLogoutRestoration() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        f.saved = f.saved.withLogoutPending(true)
        val loading = CompletableDeferred<Unit>(); val keys = CompletableDeferred<Unit>()
        val clearing = CompletableDeferred<Unit>(); val files = CompletableDeferred<Unit>()
        f.beforeLoad = { loading.complete(Unit); keys.await() }
        var view = CacheView(sync = SyncState(1, f.now))
        var effects = 0
        val runtime = AndroidLiveReconciler(f.controller, { view }, { effects++ }, { effects++ }, { effects++ }, {})
        f.controller.identityEnded = { clearing.complete(Unit); files.await(); view = CacheView(); runtime.clearIdentity() }
        try {
            val recovery = launch { recoverLivePermissions(f.controller, runtime) }
            loading.await(); runCurrent()
            assertEquals(0, effects, "Recovery must await key loading")
            keys.complete(Unit); clearing.await(); runCurrent()
            assertEquals(0, effects, "Login state alone is not completed logout cleanup")
            files.complete(Unit); recovery.join(); runCurrent()
            assertEquals(0, effects)
        } finally { keys.complete(Unit); files.complete(Unit); f.controller.close(); runCurrent() }
    }
    @Test fun bindingRetriesCacheAndNativeCleanupAsOneEpochBoundUnit() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        var fail = false; var clears = 0; var native = 0
        val files = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun clear() { clears++; if (fail) { fail = false; error("fail-once cleanup") } }
        }
        val cache = memoryCache(files) { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val runtime = AndroidLiveReconciler(f.controller, { cache.view.value }, { native++ }, { native++ }, {}, { native = 0 })
        data.bindTrust(f.controller, backgroundScope, onIdentityEnded = { runtime.clearIdentity(it) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        f.controller.restore() // Production installs durable-owner admission before restoring trust.
        try {
            cache.install(emptySnapshot(1)); runtime.reconcile(); assertEquals(2, native)
            fail = true; f.switchMember()
            f.controller.liveAdmission.first { it == 1L }
            assertTrue(clears >= 2); assertEquals(0, native, "Retry must retire alarms and notifications too")
            cache.install(emptySnapshot(2)); runtime.reconcile(); assertEquals(2, native)
            runtime.clearIdentity(0) // obsolete retry must not retire replacement inventory
            assertEquals(2, native)
            runtime.reconcile(); assertEquals(4, native, "New epoch remains admitted without restarting")
        } finally { f.controller.close(); cache.close(); runCurrent() }
    }
    @Test fun lazyRuntimeInPreClearGapStartsClosed() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        f.controller.restore()
        val clearing = CompletableDeferred<Unit>(); val finish = CompletableDeferred<Unit>()
        var effects = 0
        f.controller.identityEnded = { clearing.complete(Unit); finish.await() }
        val logout = launch { f.controller.logout() }
        try {
            clearing.await()
            val runtime = AndroidLiveReconciler(f.controller, { CacheView(sync = SyncState(1, f.now)) },
                { effects++ }, { effects++ }, { effects++ }, {})
            runtime.reconcile(recover = true)
            assertEquals(0, effects, "Lazy runtime cannot infer admission from advanced epoch")
        } finally { finish.complete(Unit); logout.join(); f.controller.close(); runCurrent() }
    }
}
