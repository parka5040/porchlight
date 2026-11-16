package app.porchlight.shared

import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.data.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.theme.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.*
import java.io.File
import java.nio.file.Files
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientSyncFixFStorageGateTest {
    @Test fun upgradePurgeFailureHasTrustPreservingRecoveryAtCompactDoubleFont() = gate(320, false)
    @Test fun restartedPurgeFailureHasTrustPreservingRecoveryAtDesktopDoubleFont() = gate(1280, true)

    private fun gate(width: Int, restart: Boolean) {
        val f = SyncBundleFixtures
        val dir = Files.createTempDirectory(File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }.toPath(), "storage-gate-").toFile()
        val database = File(dir, "read-cache.db")
        JdbcSqliteDriver("jdbc:sqlite:$database").let { driver ->
            PorchlightDb.Schema.create(driver)
            driver.execute(null, "DROP TABLE CacheFileNamespace", 0)
            driver.execute(null, "DROP TABLE CacheCleanupFailure", 0)
            driver.execute(null, "PRAGMA user_version = 13", 0)
            driver.close()
        }
        var failed = true; var attempts = 0; var clearedTrust = false; var snapshots = 0
        var ownerAdmitted = false
        var admitted: () -> Boolean = { false }
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun clear() { attempts++; if (failed) error("Storage unavailable") }
        }
        val saved = DeviceTrust("fixture-device", "leaf", "root", "fixture-session", "2027-04-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = saved
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) = Unit
            override suspend fun clear() { clearedTrust = true }
            override fun certificateExpiry(pem: String) = saved.certificateExpiresAt
        }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when {
                path == "/v1/me" -> buildJsonObject { put("member_id", "fixture-member"); put("session_expires_at", "2027-04-01T00:00:00Z") }
                path == "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
                path.startsWith("/v1/snapshot") -> {
                    check(admitted()) { "Snapshot must follow completed owner admission" }
                    snapshots++
                    WireJson.encodeToJsonElement(f.snapshot).jsonObject
                }
                path.startsWith("/v1/board") || path.startsWith("/v1/agenda") || path.startsWith("/v1/lists") -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> error("Unexpected request")
            }
        }
        val controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }, "desktop", { f.now }, false)
        admitted = { ownerAdmitted && controller.backgroundSessionUsable() }
        var cache = desktopReadCache(database, store) { f.now }
        if (restart) {
            runBlocking { assertFailsWith<IllegalStateException> { cache.initialize(CacheOwner("fixture-ca", "fixture-member", "fixture-device")) } }
            cache.close(); cache = desktopReadCache(database, store) { f.now }
        }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val binding = data.bindTrust(controller, scope, streamEnabled = MutableStateFlow(false),
            onIdentityStarting = { _, _ -> ownerAdmitted = true })
        try {
            runDesktopComposeUiTest(width = width, height = 1400) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
                    PorchlightTheme(ThemeMode.Daylight, false) {
                        androidx.compose.material3.Surface(color = androidx.compose.material3.MaterialTheme.colorScheme.background) {
                            TrustGate(controller) { Text("Household shell") }
                        }
                    }
                } }
                waitUntil(timeoutMillis = 5_000) { controller.state.value != TrustState.Loading }
                onNodeWithText("Local storage needs attention").assertExists()
                onNodeWithText("free space", substring = true).assertExists()
                onNodeWithText("Clear trust and enroll again").assertDoesNotExist()
                onNodeWithText("Household shell").assertDoesNotExist()
                assertEquals(0, snapshots, "Storage-gated owners cannot refresh")
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = java.awt.image.BufferedImage(pixels.width, pixels.height, java.awt.image.BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val evidence = File("build/test-evidence/client-sync-fix-f").apply { mkdirs() }
                assertTrue(javax.imageio.ImageIO.write(image, "png", evidence.resolve("storage-gate-$width-double.png")))
                val before = attempts
                onNodeWithText("Try again").performScrollTo().performClick()
                waitUntil(timeoutMillis = 5_000) { attempts > before && controller.state.value == TrustState.StorageRecoveryRequired && !controller.busy.value }
                assertNotNull(cache.cleanupRetryAt.value, "Failed manual retry retains the cleanup obligation")
                failed = false
                val beforeConfirmation = attempts
                onNodeWithText("Clear local cache").performScrollTo().performClick()
                onNodeWithText("Your device credentials, settings, item and comment drafts, and pending requests are kept.", substring = true).assertExists()
                assertEquals(beforeConfirmation, attempts, "Cleanup cannot start before confirmation")
                onNodeWithText("Confirm clear local cache").performClick()
                waitUntil(timeoutMillis = 5_000) { controller.state.value == TrustState.Ready && cache.sync.value != null }
                onNodeWithText("Household shell").assertExists()
                assertTrue(snapshots > 0)
                assertFalse(clearedTrust)
                assertEquals(saved.session, controller.sessionToken())
                assertNull(cache.cleanupRetryAt.value)
            }
        } finally { binding.cancel(); scope.cancel(); controller.close(); data.close(); dir.deleteRecursively() }
    }
}
