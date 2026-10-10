package app.porchlight.shared.update

import app.porchlight.shared.PlatformKind
import app.porchlight.shared.api.models.*
import app.porchlight.shared.backups.DownloadHead
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.settings.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.hours

fun updateRelease(version: String = "0.1.7", files: List<ReleaseFile> = emptyList()) = ClientRelease(version,
    AndroidRelease("porchlight-$version.apk", 3, "a".repeat(64), "app.porchlight", releaseVersionCode(version) ?: 0, listOf("b".repeat(64))),
    DesktopRelease("porchlight-desktop-$version-linux-x86_64.tar.gz", files))
class UpdateSettings : SettingsStore {
    override val settings = MutableStateFlow(DeviceSettings())
    override suspend fun saveRoute(route: AppRoute) {}
    override suspend fun setTheme(mode: ThemeMode) {}
    override suspend fun setStartScreen(screen: StartScreen) {}
    override suspend fun setLimitedReminders(limited: Boolean) {}
    override suspend fun dismissUpdate(version: String) { settings.value = settings.value.copy(dismissedUpdate = version) }
}
open class TestUpdateSource(var value: ClientRelease = updateRelease()) : UpdateSource {
    var checks = 0
    var failure: Exception? = null
    override suspend fun release(): ClientRelease { checks++; failure?.let { throw it }; return value }
    override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) { error("unused") }
}
open class TestUpdatePlatform : UpdatePlatform {
    override val kind = PlatformKind.Desktop
    var reason: String? = null
    var installs = 0
    override suspend fun unavailable(release: ClientRelease) = reason
    override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState {
        installs++; report(UpdateState.Downloading(3, 3)); report(UpdateState.Verifying); report(UpdateState.Installing)
        return UpdateState.ReadyToRestart
    }
}
@OptIn(ExperimentalCoroutinesApi::class)
class UpdateControllerTest {
    @Test fun versionsAndUnavailableAnd404AreFailClosed() = runTest {
        val source = TestUpdateSource(); val platform = TestUpdatePlatform()
        val controller = UpdateController(source, platform, UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        controller.checkNow(); assertIs<UpdateState.Available>(controller.state.value)
        source.value = updateRelease("0.1.6"); controller.checkNow(); assertEquals(UpdateState.Idle, controller.state.value)
        source.value = updateRelease("0.1.5"); controller.checkNow(); assertEquals(UpdateState.Idle, controller.state.value)
        source.failure = ApiProblem("client-release-unavailable", 404, "Unavailable")
        controller.checkNow(); assertEquals(UpdateState.Idle, controller.state.value)
        source.failure = null; source.value = updateRelease()
        platform.reason = "Install minisign"; controller.checkNow(); assertEquals(UpdateState.Unavailable("Install minisign"), controller.state.value)
        assertNull(controller.update()); assertEquals(0, platform.installs)
        source.value = updateRelease().copy(android = updateRelease().android.copy(versionCode = 108))
        controller.checkNow(); assertIs<UpdateState.Failed>(controller.state.value)
    }
    @Test fun checksAreHourlyUnlessExplicitOrReconnectAndDismissalIsPerVersion() = runTest {
        var now = Instant.fromEpochSeconds(0); val source = TestUpdateSource(); val settings = UpdateSettings()
        val controller = UpdateController(source, TestUpdatePlatform(), settings, backgroundScope, { now }, 106, "0.1.6")
        controller.checkNow(false); controller.checkNow(false); assertEquals(1, source.checks)
        now += 1.hours; controller.checkNow(false); assertEquals(2, source.checks)
        controller.checkNow(); assertEquals(3, source.checks)
        controller.dismiss("0.1.7").join(); assertEquals("0.1.7", settings.settings.value.dismissedUpdate)
        assertNotEquals("0.1.8", settings.settings.value.dismissedUpdate)
        val disabled = UpdateController(source, TestUpdatePlatform(), settings, backgroundScope, { now }, 0, "0.1.6-dev")
        disabled.checkNow(); assertIs<UpdateState.Unavailable>(disabled.state.value); assertEquals(3, source.checks)
    }
    @Test fun onlyOneInstallRunsAndCheckCannotReplaceItsState() = runTest {
        val controller = UpdateController(TestUpdateSource(), TestUpdatePlatform(), UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        controller.checkNow()
        val job = assertNotNull(controller.update()); assertNull(controller.update())
        job.join(); assertEquals(UpdateState.ReadyToRestart, controller.state.value)
        controller.checkNow(); assertEquals(UpdateState.ReadyToRestart, controller.state.value)
    }
    @Test fun cancellationReturnsAvailableAndResetFencesLateCompletions() = runTest {
        val platform = object : TestUpdatePlatform() {
            override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState {
                report(UpdateState.Downloading(1, 10)); awaitCancellation()
            }
        }
        val controller = UpdateController(TestUpdateSource(), platform, UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        controller.checkNow(); val first = assertNotNull(controller.update()); runCurrent()
        assertIs<UpdateState.Downloading>(controller.state.value)
        controller.cancel(); first.join(); assertIs<UpdateState.Available>(controller.state.value)
        val second = assertNotNull(controller.update()); runCurrent(); controller.reset(); second.join()
        assertEquals(UpdateState.Idle, controller.state.value)
    }
    @Test fun cancellingBeforeTheFirstDispatchReleasesTheOperationLock() = runTest {
        val controller = UpdateController(TestUpdateSource(), TestUpdatePlatform(), UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        controller.checkNow()
        val first = assertNotNull(controller.update()); controller.cancel(); first.join()
        assertNotNull(controller.update()).join()
        assertEquals(UpdateState.ReadyToRestart, controller.state.value)
    }
    @Test fun needsPermissionContinuesOnResumeAndFailureIsVisible() = runTest {
        val platform = object : TestUpdatePlatform() {
            override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit) = UpdateState.NeedsPermission
            override suspend fun resumePermission(report: (UpdateState) -> Unit) = UpdateState.Installing
        }
        val controller = UpdateController(TestUpdateSource(), platform, UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        controller.checkNow(); controller.update()!!.join(); assertEquals(UpdateState.NeedsPermission, controller.state.value)
        controller.onResume(); runCurrent(); assertEquals(UpdateState.Installing, controller.state.value)
        val failing = object : TestUpdatePlatform() {
            override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState = throw UpdateFailure(DOWNLOAD_MISMATCH)
        }
        val other = UpdateController(TestUpdateSource(), failing, UpdateSettings(), backgroundScope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        other.checkNow(); other.update()!!.join(); assertEquals(UpdateState.Failed(DOWNLOAD_MISMATCH), other.state.value)
    }
    @Test fun strictVersionAndPathsRejectTraversalAndEncodedSeparators() {
        assertEquals(106L, releaseVersionCode("0.1.6"))
        for (version in listOf("dev", "0.1.6-beta", "1.100.0", "999999999999999.0.0")) assertNull(releaseVersionCode(version))
        for (path in listOf("../install.sh", "/install.sh", "lib//ops.py", "lib/../ops.py", "lib%2fops.py", "lib\\ops.py", "lib/a?x=y"))
            assertFailsWith<IllegalArgumentException> { requireReleasePath(path) }
        requireReleasePath("lib/operations.py")
    }
    @Test fun recoveryReleaseUsesDeviceWithoutSessionEvenBehind426() = runTest {
        val paths = mutableListOf<String>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): kotlinx.serialization.json.JsonObject {
                assertNull(session); paths += path
                return WireJson.encodeToJsonElement(ClientRelease.serializer(), updateRelease()) as kotlinx.serialization.json.JsonObject
            }
            override suspend fun download(path: String, session: String?, range: String?, ifRange: String?, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                assertNull(session); assertEquals("bytes=1-", range); assertEquals("\"hash\"", ifRange); paths += path
            }
        }
        val stored = DeviceTrust("device", "leaf", "root", null, null, listOf("https://lan"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = stored
            override suspend fun csr() = "unused"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }
        val trust = TrustController(keys, factory, "desktop", { Instant.parse("2026-10-10T00:00:00Z") }, false, StandardTestDispatcher(testScheduler))
        try {
            trust.restore(); trust.onProblem(ApiProblem("upgrade-required", 426, "Upgrade"))
            val source = TrustedUpdateSource(trust)
            assertEquals("0.1.7", source.release().version)
            source.download(ReleaseFile("lib/operations.py", 3, "hash"), 1, {}, {})
            assertNull(source.server())
            assertEquals(listOf("/v1/client-release", "/v1/client-release/files/lib/operations.py"), paths)
            assertIs<TrustState.UpgradeRequired>(trust.state.value)
        } finally { trust.close(); runCurrent() }
    }
}
