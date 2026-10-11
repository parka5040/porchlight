package app.porchlight.shared

import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.settings.*
import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import kotlinx.coroutines.flow.first

@OptIn(ExperimentalTestApi::class)
class C2aWiringTest {
    @Test fun desktopTrustAndOnboardingPrecedeRestoredAccountShell() = runComposeUiTest {
        var load: DeviceTrust? = null
        val stored = DeviceTrust("device", "leaf", "root", "session", "2027-04-01T00:00:00Z", listOf("https://lan"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = load
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
        }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {
                when(path) {
                    "/v1/me" -> put("member_id", "member")
                    "/v1/devices" -> put("devices", JsonArray(emptyList()))
                    else -> put("endpoints", JsonArray(listOf(JsonPrimitive("https://lan"))))
                }
            }
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }
        val controller = TrustController(keys, factory, "desktop", { Instant.parse("2026-10-05T00:00:00Z") }, false)
        val services = FakeServices(DeviceSettings(currentRoute = AppRoute.Settings(app.porchlight.shared.navigation.SettingsSection.Account)))
        var permit by androidx.compose.runtime.mutableStateOf(false)
        setContent { TrustedPorchlightApp(services, controller, onboarding = { content -> if (permit) content() else Text("Permission checkpoint") }) }
        onNodeWithTag("shell-content").assertDoesNotExist()
        // The launch restore runs off the Compose dispatcher, so idling alone does not cover it.
        waitUntil(timeoutMillis = 5_000) { onAllNodesWithText("Enroll this device").fetchSemanticsNodes().isNotEmpty() }
        runOnIdle { load = stored }
        kotlinx.coroutines.runBlocking { controller.restore() }
        onNodeWithText("Permission checkpoint").assertExists()
        onNodeWithTag("shell-content").assertDoesNotExist()
        runOnIdle { permit = true }
        onNodeWithTag("shell-content").assertExists()
        onNodeWithText("Your devices").assertExists()
        kotlinx.coroutines.runBlocking { controller.onTransportError(DeviceCertificateRevoked()) }
        onNodeWithTag("shell-content").assertDoesNotExist()
    }

    @Test fun localTrustRecoveryRetriesWithoutClearing() = runComposeUiTest {
        var attempts = 0
        var cleared = false
        val keys = object : DeviceKeyStore {
            override suspend fun load(): DeviceTrust? { attempts++; if (attempts == 1) throw java.io.IOException(); return null }
            override suspend fun clear() { cleared = true }
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri): ApiTransport = error("unused")
            override suspend fun authenticated(trust: DeviceTrust): ApiTransport = error("unused")
        }, "desktop", { Instant.parse("2026-10-05T00:00:00Z") }, false)
        setContent { TrustGate(controller) { Text("Private") } }
        onNodeWithText("Try again").performScrollTo().performClick()
        waitUntil(timeoutMillis = 5_000) { onAllNodesWithText("Enroll this device").fetchSemanticsNodes().isNotEmpty() }
        assertFalse(cleared)
        assertEquals(2, attempts)
    }

    @Test fun relockKeepsBoardAndPersistedResumeRoute() = runComposeUiTest {
        val root = java.io.File(System.getProperty("porchlight.test.dir")).also { it.mkdirs() }
        val dir = java.nio.file.Files.createTempDirectory(root.toPath(), "relock-").toFile()
        val job = kotlinx.coroutines.SupervisorJob()
        val store = DataStoreSettingsStore(androidx.datastore.preferences.core.PreferenceDataStoreFactory.create(
            scope = kotlinx.coroutines.CoroutineScope(job + kotlinx.coroutines.Dispatchers.IO),
            produceFile = { java.io.File(dir, "device.preferences_pb") }))
        val base = FakeServices()
        val services = object : AppServices {
            override val settings = store
            override val clock = base.clock
            override val platform = base.platform
        }
        var unlocked by androidx.compose.runtime.mutableStateOf(true)
        try {
            setContent { PorchlightApp(services, gate = { if (unlocked) it() else Text("Locked") }) }
            waitUntil { onAllNodesWithContentDescription("Navigate to Board").fetchSemanticsNodes().isNotEmpty() }
            onNodeWithContentDescription("Navigate to Board").performClick()
            onNodeWithContentDescription("Navigate to Board").assertIsSelected()
            // The route is saved by ShellNavigation's LaunchedEffect collector, which runs on this (test) thread:
            // poll with waitUntil, which keeps pumping the UI loop, instead of blocking the thread the save needs.
            waitUntil(timeoutMillis = 10_000) { kotlinx.coroutines.runBlocking { store.settings.first() }.currentRoute == AppRoute.Board() }
            runOnIdle { unlocked = false }
            onNodeWithTag("shell-content").assertDoesNotExist()
            runOnIdle { unlocked = true }
            onNodeWithContentDescription("Navigate to Board").assertIsSelected()
            waitForIdle()
            assertEquals(AppRoute.Board(), kotlinx.coroutines.runBlocking { store.settings.first() }.currentRoute)
        } finally { job.cancel(); kotlinx.coroutines.runBlocking { job.join() }; dir.deleteRecursively() }
    }

    @Test fun defaultPinGateCannotUnlock() = runComposeUiTest {
        val stored = DeviceTrust("device", "leaf", "root", "session", "2027-04-01T00:00:00Z", listOf("https://lan"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = stored
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
        }
        val controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri): ApiTransport = error("unused")
            override suspend fun authenticated(trust: DeviceTrust) = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
            }
        }, "android", { Instant.parse("2026-10-05T00:00:00Z") }, true)
        setContent { TrustedPorchlightApp(FakeServices(), controller) }
        // The controller restores stored trust on its own scope, outside Compose idling: wait for the gate.
        waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Unlock with device credential").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithText("Unlock with device credential").performClick()
        onNodeWithTag("shell-content").assertDoesNotExist()
        assertEquals(TrustState.PinRequired, controller.state.value)
        controller.close()
    }

    @Test fun reminderReadFailureEmitsSafeDefaultAndReportsFailure() = runTest {
        val failure = java.io.IOException("fixture read failure")
        var reported: Throwable? = null
        val settings = kotlinx.coroutines.flow.flow<DeviceSettings> { throw failure }
            .reminderSettingsOrDefault { reported = it }.first()
        assertFalse(settings.limitedReminders)
        assertSame(failure, reported)
    }

    @Test fun limitedRemindersSurviveDataStoreReopen() = runTest {
        val root = java.io.File(System.getProperty("porchlight.test.dir")).also { it.mkdirs() }
        val dir = java.nio.file.Files.createTempDirectory(root.toPath(), "limited-").toFile()
        val file = java.io.File(dir, "device.preferences_pb")
        var job = kotlinx.coroutines.SupervisorJob()
        fun store() = DataStoreSettingsStore(androidx.datastore.preferences.core.PreferenceDataStoreFactory.create(
            scope = kotlinx.coroutines.CoroutineScope(job + kotlinx.coroutines.Dispatchers.IO), produceFile = { file }))
        try {
            store().setLimitedReminders(true)
            job.cancel(); job.join()
            job = kotlinx.coroutines.SupervisorJob()
            val restored = store().settings.first()
            assertTrue(restored.limitedReminders)
        } finally { job.cancel(); job.join(); dir.deleteRecursively() }
    }
}
