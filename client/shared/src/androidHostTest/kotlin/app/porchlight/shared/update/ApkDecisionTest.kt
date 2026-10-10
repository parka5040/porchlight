package app.porchlight.shared.update

import kotlin.test.*

class ApkDecisionTest {
    private val own = ApkIdentity("app.porchlight", 106, setOf("a".repeat(64)))
    private val advertised = own.copy(versionCode = 107)
    @Test fun preflightRequiresPackageAndExactNonemptySignerSet() {
        assertNull(apkPreflight(own, advertised))
        assertNotNull(apkPreflight(own, advertised.copy(packageName = "app.other")))
        assertNotNull(apkPreflight(own, advertised.copy(signers = setOf("b".repeat(64)))))
        assertNotNull(apkPreflight(own.copy(signers = emptySet()), advertised.copy(signers = emptySet())))
    }
    @Test fun archiveRequiresPackageVersionAndSignersButAllowsMissingArchiveSigningInfo() {
        assertNull(apkDecision(own, advertised, advertised))
        assertNull(apkDecision(own, advertised, advertised.copy(signers = null)))
        assertNotNull(apkDecision(own, advertised, null))
        assertNotNull(apkDecision(own, advertised, advertised.copy(packageName = "app.other")))
        assertNotNull(apkDecision(own, advertised, advertised.copy(versionCode = 106)))
        assertNotNull(apkDecision(own, advertised, advertised.copy(versionCode = 108)))
        assertNotNull(apkDecision(own, advertised, advertised.copy(signers = emptySet())))
        assertNotNull(apkDecision(own, advertised, advertised.copy(signers = setOf("b".repeat(64)))))
        assertNotNull(apkDecision(own, advertised.copy(signers = setOf("b".repeat(64))), advertised))
        assertNotNull(apkDecision(own, own, own))
    }
    @Test fun processReplacementDoesNotLeaveAStaleInstallingState() {
        assertEquals(UpdateState.Idle, restoredInstallState(107, 107, false))
        assertEquals(UpdateState.Idle, restoredInstallState(108, 107, true))
        assertEquals(UpdateState.Installing, restoredInstallState(106, 107, true))
        assertIs<UpdateState.Failed>(restoredInstallState(106, 107, false))
    }
    @Test fun installerStatusMappingIsFailClosed() {
        assertEquals(InstallResult.Confirm, installResult(-1, null))
        assertEquals(InstallResult.Success, installResult(0, null))
        for (status in 1..8) assertIs<InstallResult.Failure>(installResult(status, "Refused"))
        assertIs<InstallResult.Failure>(installResult(-99, null))
        assertFalse((installResult(1, "bad\nmessage") as InstallResult.Failure).reason.contains('\n'))
    }
}
