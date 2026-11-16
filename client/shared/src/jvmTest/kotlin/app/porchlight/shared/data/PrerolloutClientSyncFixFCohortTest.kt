package app.porchlight.shared.data

import kotlin.test.Test

class PrerolloutClientSyncFixFCohortTest {
    @Test fun replacementBindingDoesNotRacePendingSnapshots() {
        repeat(20) { PrerolloutClientDataFix1CohortTest().replacementTokenBindingDoesNotInheritObsoleteFlightCancellation() }
    }
}
