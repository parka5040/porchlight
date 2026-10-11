package app.porchlight.shared.trust

import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiAccountTest {
    @Test fun logoutCompletesLocalCleanupAfterAccountRemoval() = lifecycle(password = false)
    @Test fun passwordOutcomeSurvivesAccountRemoval() = lifecycle(password = true)
    private fun lifecycle(password: Boolean) = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        var stored: DeviceTrust? = DeviceTrust("device", "leaf", "root", "fixture-session", "2027-04-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = stored
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { stored = trust }
            override suspend fun clear() { stored = null }
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val entered = AtomicBoolean(); val finished = AtomicBoolean(); val release = CompletableDeferred<Unit>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path == if (password) "/v1/auth/password" else "/v1/auth/logout") {
                    entered.set(true); release.await(); finished.set(true); return buildJsonObject {}
                }
                return buildJsonObject { put("member_id", "self"); put("devices", JsonArray(emptyList())); put("endpoints", buildJsonArray { add("https://example.invalid") }) }
            }
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val cache = memoryCache { now }
        try {
            controller.restore()
            cache.inSession { cache.putItem(it, itemResponse("private")) }
            controller.identityEnded = { cache.clear() }
            runDesktopComposeUiTest(width = 320, height = 1400) {
                var visible by mutableStateOf(true)
                setContent { PorchlightTheme(ThemeMode.Daylight, false) {
                    if (visible) TrustAccountScreen(controller)
                } }
                waitUntil { runCatching { onNodeWithText("Log out").assertIsEnabled() }.isSuccess }
                if (password) {
                    onNodeWithText("Change password").performScrollTo().performClick()
                    onNodeWithText("Current password").performTextInput("old-fixture-password")
                    onNodeWithText("New password").performTextInput("new-fixture-password")
                    onNodeWithText("Save password").performScrollTo().performClick()
                } else onNodeWithText("Log out").performScrollTo().performClick()
                waitUntil { entered.get() }
                runOnIdle { visible = false }; waitForIdle()
                release.complete(Unit)
                // A controller-owned action keeps busy until its local cleanup/outcome is committed.
                waitUntil { !controller.busy.value }
                assertTrue(finished.get(), "account request was cancelled when composition disappeared")
                if (password) {
                    runOnIdle { visible = true }
                    waitUntil { onAllNodesWithText("Password changed.").fetchSemanticsNodes().isNotEmpty() }
                } else {
                    waitUntil { controller.state.value == TrustState.Login }
                    assertNull(stored?.session)
                    assertTrue(cache.view.value.items.isEmpty(), "logout clears household cache")
                }
            }
        } finally { release.complete(Unit); controller.close(); cache.close() }
    }
}
