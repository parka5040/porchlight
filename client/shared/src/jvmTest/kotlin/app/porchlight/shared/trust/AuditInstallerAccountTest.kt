package app.porchlight.shared.trust

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.runBlocking
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import java.awt.image.BufferedImage
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import javax.imageio.ImageIO
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class AuditInstallerAccountTest(private val platform: String, private val action: String) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{0}-{1}")
        fun cases() = listOf("android", "desktop").flatMap { platform ->
            listOf("logout", "password", "reauth", "revoke").map { arrayOf(platform, it) }
        }
    }

    @Test fun accountMaintenanceRetriesOriginalRequest() {
        val failedPath = when (action) {
            "revoke" -> "/v1/devices/tablet/revoke"
            else -> "/v1/auth/$action"
        }
        data class Attempt(val path: String, val body: String, val key: String?, val stepUp: String?)
        // The mock engine appends on its own thread while waitUntil iterates: iteration must not need the list lock.
        val attempts = CopyOnWriteArrayList<Attempt>()
        var generated = 0
        val http = HttpClient(MockEngine { request ->
            val path = request.url.encodedPath
            if (request.method != HttpMethod.Get) {
                attempts += Attempt(path, request.body.toByteArray().decodeToString(), request.headers["Idempotency-Key"], request.headers["Porchlight-Step-Up"])
                if (path == failedPath && attempts.count { it.path == path } == 1) return@MockEngine respond(
                    """{"type":"urn:porchlight:problem:maintenance","status":503,"title":"Server is updating"}""",
                    HttpStatusCode.ServiceUnavailable, headersOf("Content-Type" to listOf("application/problem+json"), "Retry-After" to listOf("1")))
            }
            val body = when (path) {
                "/v1/me" -> """{"member_id":"member","session_expires_at":"2027-04-01T00:00:00Z"}"""
                "/v1/endpoints" -> """{"endpoints":["https://fixture.invalid"]}"""
                "/v1/devices" -> """{"devices":[{"id":"tablet","member_id":"member","label":"Tablet","platform":"android","created_at":"2026-10-06T12:00:00Z","current":false,"revoked_at":null}],"items":[],"next_cursor":null}"""
                "/v1/auth/reauth" -> """{"step_up_token":"fixture-step-up"}"""
                else -> "{}"
            }
            respond(body, HttpStatusCode.OK, headersOf("Content-Type", "application/json"))
        })
        val api = KtorApiTransport(http, listOf("https://fixture.invalid"), HeaderPolicy(ClientInfo("0.1.0"), { "UTC" }, { "generated-${++generated}" }), { false })
        val stored = DeviceTrust("self", "leaf", "root", "fixture-session", "2027-04-01T00:00:00Z", listOf("https://fixture.invalid"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = stored
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }
        val controller = TrustController(keys, factory, platform, { Instant.parse("2026-10-06T00:00:00Z") }, false)
        try {
            runBlocking { controller.restore() }
            runDesktopComposeUiTest(width = 320, height = 1000) {
                setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { TrustAccountScreen(controller) } } }
                waitUntil(timeoutMillis = 5000) { onAllNodesWithText("Tablet").fetchSemanticsNodes().isNotEmpty() }
                when (action) {
                    "logout" -> onNodeWithText("Log out").performScrollTo().performClick()
                    "password" -> {
                        onNodeWithText("Change password").performScrollTo().performClick()
                        onNodeWithText("Current password").performTextInput("fixture current password")
                        onNodeWithText("New password").performTextInput("fixture replacement password")
                        onNodeWithText("Save password").performScrollTo().performClick()
                    }
                    else -> {
                        onNodeWithContentDescription("Revoke Tablet").performScrollTo().performClick()
                        onNodeWithText("Password").performTextInput("fixture current password")
                        onNodeWithText("Confirm").performClick()
                    }
                }
                waitUntil(timeoutMillis = 5000) { attempts.any { it.path == failedPath } }
                waitUntil(timeoutMillis = 5000) { onAllNodesWithText("Retry").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Server is updating — try again shortly").assertIsDisplayed()
                onNodeWithText("Action failed. Check your connection and retry.").assertDoesNotExist()
                val pixels = onRoot().captureToImage().toPixelMap()
                val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
                for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
                val dir = File("build/test-evidence/audit-installer-account").apply { mkdirs() }
                assertTrue(ImageIO.write(image, "png", dir.resolve("$platform-$action.png")))
                onNodeWithText("Retry").performClick()
                waitUntil(timeoutMillis = 5000) { attempts.count { it.path == failedPath } == 2 }
                waitUntil(timeoutMillis = 5000) { onAllNodesWithText("Server is updating — try again shortly").fetchSemanticsNodes().isEmpty() }
                val retried = attempts.filter { it.path == failedPath }
                assertTrue(retried[0] == retried[1], "retry changed the request intent, body, key or step-up")
                if (action == "revoke") assertEquals(1, attempts.count { it.path == "/v1/auth/reauth" })
            }
        } finally { controller.close(); http.close() }
    }
}
