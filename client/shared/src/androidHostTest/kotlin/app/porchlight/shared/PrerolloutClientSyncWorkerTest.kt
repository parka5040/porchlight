package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import java.io.File
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncWorkerTest {
    @Test fun workerWithoutNetworkStillReconcilesAlarmsAndNotificationsWithoutInvokingNetwork() = runTest {
        var reconciliations = 0
        assertTrue(householdWorkerPass(network = false, reconcile = { reconciliations++ },
            synchronize = { error("Offline local worker must not invoke network") }))
        assertEquals(1, reconciliations)
    }
    @Test fun networkWorkerReconcilesBeforeAndAfterCatchUpButRetriesAnIncompleteReplay() = runTest {
        for (completed in listOf(false, true)) {
            val order = mutableListOf<String>()
            assertEquals(completed, householdWorkerPass(network = true, reconcile = { order += "local" },
                synchronize = { order += "catch-up"; completed }))
            assertEquals(if (completed) listOf("local", "catch-up", "local") else listOf("local", "catch-up"), order)
        }
    }
    @Test fun workerCancellationStillPropagates() = runTest {
        assertFailsWith<TimeoutCancellationException> {
            withTimeout(10) { householdWorkerPass(true, {}, { awaitCancellation() }) }
        }
    }
    @Test fun workManagerNetworkConstraintsCoverPeriodicAndFallbackButNotLocalReconciliation() {
        // Android framework/WorkManager dispatch is controller-only; check the actual builders without Robolectric.
        val source = File("src/androidMain/kotlin/app/porchlight/shared/live/AndroidLive.kt").readText()
        assertTrue(source.contains("setRequiredNetworkType(NetworkType.CONNECTED)"))
        assertTrue(source.contains("\"household-sync\", ExistingPeriodicWorkPolicy.UPDATE"), "Upgrade existing persisted work too")
        assertTrue(source.contains("PeriodicWorkRequestBuilder<HouseholdSyncWorker>(15, TimeUnit.MINUTES).setConstraints(syncNetworkConstraint())"))
        assertTrue(source.contains("OneTimeWorkRequestBuilder<HouseholdSyncWorker>().setConstraints(syncNetworkConstraint())"))
        assertTrue(source.contains("PeriodicWorkRequestBuilder<HouseholdReconcileWorker>(15, TimeUnit.MINUTES).build()"))
        assertTrue(source.contains("OneTimeWorkRequestBuilder<HouseholdReconcileWorker>().build()"))
        assertTrue(source.contains("backgroundSafetyNet("), "The network worker must use the shared catch-up path")
        val worker = source.substringAfter("class HouseholdSyncWorker").substringBefore("class HouseholdReconcileWorker")
        assertTrue(worker.contains("services.data.cache.inSession {"), "The whole network pass, including acknowledgement, needs its original cache lease")
    }
}
