package app.porchlight.shared.trust

import kotlin.test.Test

/** Reuse production-backend smoke cases on the bundled JVM (no test-only ZIP filesystem provider). */
class AuditDesktopPackagedBackendTest {
    @Test fun fileFallbackRoundTrips() = DesktopKeyBackendTest().noSecretServiceProviderEnrollsIntoOwnerOnlyKeyFile()
    @Test fun absentSessionBusUsesFile() = DesktopKeyBackendTest().noSessionBusCountsAsNoProvider()
    @Test fun lockedProviderUnlocks() = DesktopKeyBackendTest().lockedCollectionRequestsUnlockAtEnrollmentAndStartup()
    @Test fun dismissedUnlockNeverFallsBack() = DesktopKeyBackendTest().dismissedUnlockIsRecoverableWalletLockedWithoutRepromptOrFallback()
    @Test fun recordedFileSurvivesProviderAppearance() = DesktopKeyBackendTest().fileDeviceKeepsTheFileWhenAProviderAppears()
}
