package app.porchlight.shared.testing

import android.content.pm.ApplicationInfo
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import app.porchlight.shared.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.settings.*
import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.*
import kotlin.time.Instant

/**
 * Deterministic instrumented-test support, called only by the debug app's ShellFixtureActivity.
 * No real keys, persistence, network or household content. Fails closed in non-debuggable apps;
 * production entry points never call this function. Kept here to use shared's existing dependencies.
 */
@Composable
fun ShellFixture() {
    check(LocalContext.current.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
        "ShellFixture requires a debuggable application"
    }
    PorchlightAndroidRoot(fixtureServices, fixtureTrust.controller,
        credentialGate = { unlock -> Button(onClick = unlock) { Text("Fixture device credential") } },
        onboarding = { it() })
}

// Process-local settings deliberately survive activity recreation; never written to device storage.
private val deviceSettings = MutableStateFlow(DeviceSettings())

private val fixtureServices: AppServices = object : AppServices {
    override val settings = object : SettingsStore {
        override val settings = deviceSettings
        override suspend fun saveRoute(route: AppRoute) { deviceSettings.value = deviceSettings.value.copy(currentRoute = route) }
        override suspend fun setTheme(mode: ThemeMode) { deviceSettings.value = deviceSettings.value.copy(themeMode = mode) }
        override suspend fun setStartScreen(screen: StartScreen) { deviceSettings.value = deviceSettings.value.copy(startScreen = screen) }
        override suspend fun setLimitedReminders(limited: Boolean) { deviceSettings.value = deviceSettings.value.copy(limitedReminders = limited) }
    }
    override val clock = AppClock { Instant.parse("2026-10-05T00:00:00Z") }
    override val platform = object : PlatformInfo {
        override val kind = PlatformKind.Android
        override val systemDark = MutableStateFlow(false)
    }
}

private val fixtureTrust = AppTrustHolder({ fixtureKeys() }) { fixtureController(fixtureServices, it) }

/** Debug activity exposes these to instrumentation without duplicating the production root. */
fun fixtureControllerIdentity(): Any = fixtureTrust.controller
fun fixtureResumeRoute(): AppRoute = deviceSettings.value.currentRoute

private fun fixtureKeys(): DeviceKeyStore {
    val stored = DeviceTrust("fixture", "not-a-certificate", "not-a-root", "fixture-only", "2027-04-01T00:00:00Z", listOf("https://fixture.invalid"), "2027-10-01T00:00:00Z")
    return object : DeviceKeyStore {
        override suspend fun load() = stored
        override suspend fun csr() = error("No enrollment in fixture")
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
        override suspend fun clear() {}
        override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
    }
}

private fun fixtureController(services: AppServices, keys: DeviceKeyStore): TrustController {
    val api = object : ApiTransport {
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {
            if (path == "/v1/me") put("member_id", "fixture-member")
            else if (path == "/v1/devices") put("devices", JsonArray(emptyList()))
            else put("endpoints", JsonArray(listOf(JsonPrimitive("https://fixture.invalid"))))
        }
    }
    val factory = object : TrustTransportFactory {
        override fun enrollment(uri: EnrollUri) = api
        override suspend fun authenticated(trust: DeviceTrust) = api
    }
    return TrustController(keys, factory, "android", { services.clock.now() }, true)
}
