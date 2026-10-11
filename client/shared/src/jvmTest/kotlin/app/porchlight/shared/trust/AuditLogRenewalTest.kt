package app.porchlight.shared.trust

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.awt.image.BufferedImage
import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.security.KeyPair
import javax.imageio.ImageIO
import kotlin.test.*
import kotlin.time.Clock

@OptIn(ExperimentalTestApi::class)
class AuditLogRenewalTest {
    private class StoredKeys : DeviceKeyStore {
        private var encoded = WireJson.encodeToString(DeviceTrust("device", "old-leaf", "ca", "session", "2030-01-01T00:00:00Z",
            listOf("https://localhost:8443"), "2026-10-20T00:00:00Z"))
        var rejectPending = false
        var rejectPublication = false
        var csrCalls = 0
        override suspend fun load() = WireJson.decodeFromString<DeviceTrust>(encoded)
        override suspend fun csr() = "fixture-csr-${++csrCalls}"
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) {
            if (rejectPending && trust.pendingRenewal != null) throw IOException("Pending write failed")
            if (rejectPublication && trust.certificate == "renewed-leaf") throw IOException("Publication failed")
            encoded = WireJson.encodeToString(trust)
        }
        override suspend fun clear() = Unit
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private class Server(private val keys: StoredKeys) : TrustTransportFactory, ApiTransport {
        val requests = mutableListOf<Pair<String?, JsonObject?>>()
        var started: CompletableDeferred<Unit>? = null
        var release: CompletableDeferred<Unit>? = null
        var failure: ApiProblem? = null
        override fun enrollment(uri: EnrollUri): ApiTransport = error("Already enrolled")
        override suspend fun authenticated(trust: DeviceTrust): ApiTransport = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path) {
            "/v1/me" -> jsonBody("member_id" to "member")
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://localhost:8443")))) }
            "/v1/auth/logout" -> buildJsonObject {}
            "/v1/auth/login" -> buildJsonObject { put("session", jsonBody("token" to "new-session", "expires_at" to "2030-01-01T00:00:00Z")) }
            else -> {
                val pending = assertNotNull(keys.load().pendingRenewal, "Attempt must be durable before dispatch")
                assertTrue(pending.idempotencyKey == idempotencyKey && pending.csr == body?.get("csr")?.jsonPrimitive?.content)
                assertEquals(keys.enrollmentKeyAlias, pending.keyAlias)
                requests += idempotencyKey to body
                started?.complete(Unit)
                release?.await()
                failure?.let { throw it }
                jsonBody("certificate" to "renewed-leaf")
            }
        }
    }
    private fun controller(keys: StoredKeys, server: Server) = TrustController(keys, server, "desktop",
        { kotlin.time.Instant.parse("2026-10-11T00:00:00Z") }, false)

    @Test fun pendingWriteFailureSendsNothingAndPublicationFailureRetainsAttempt() = runBlocking {
        val keys = StoredKeys()
        val server = Server(keys)
        val first = controller(keys, server)
        try {
            first.restore()
            keys.rejectPending = true
            assertFailsWith<IOException> { first.refreshEndpointsAndRenew() }
            assertEquals(0, server.requests.size)
            keys.rejectPending = false
            keys.rejectPublication = true
            assertFailsWith<IOException> { first.refreshEndpointsAndRenew() }
            assertEquals("old-leaf", keys.load().certificate)
            assertNotNull(keys.load().pendingRenewal)
        } finally { first.close() }
        keys.rejectPublication = false
        val retry = controller(keys, server)
        try {
            retry.restore(); retry.refreshEndpointsAndRenew()
            assertTrue(server.requests[0] == server.requests[1])
            assertNull(keys.load().pendingRenewal)
            assertEquals("renewed-leaf", keys.load().certificate)
        } finally { retry.close() }
    }

    @Test fun lateReplyAfterLogoutLoginIsFencedButItsPendingIdentitySurvives() = runBlocking {
        val keys = StoredKeys()
        val server = Server(keys)
        val gate = controller(keys, server)
        try {
            gate.restore()
            server.started = CompletableDeferred(); server.release = CompletableDeferred()
            val refresh = async { gate.refreshEndpointsAndRenew() }
            server.started!!.await()
            gate.logout(); gate.login("fixture phrase")
            server.release!!.complete(Unit); refresh.await()
            assertEquals("old-leaf", keys.load().certificate)
            assertNotNull(keys.load().pendingRenewal)
            gate.refreshEndpointsAndRenew()
            assertTrue(server.requests[0] == server.requests[1])
            assertEquals("new-session", keys.load().session)
            assertEquals(1, keys.csrCalls)
            assertNull(keys.load().pendingRenewal)
        } finally { gate.close() }
    }

    @Test fun preHandlerRateLimitRetainsAmbiguousAttemptButTooEarlyClearsSealedRejection() = runBlocking {
        val keys = StoredKeys()
        val server = Server(keys)
        val gate = controller(keys, server)
        try {
            gate.restore()
            server.failure = ApiProblem("rate-limited", 429, "Retry later")
            assertFailsWith<ApiProblem> { gate.refreshEndpointsAndRenew() }
            val pending = assertNotNull(keys.load().pendingRenewal)
            server.failure = ApiProblem("renewal-too-early", 409, "Not due")
            gate.refreshEndpointsAndRenew()
            assertTrue(server.requests[0] == server.requests[1])
            assertNull(keys.load().pendingRenewal)
            server.failure = null
            gate.refreshEndpointsAndRenew()
            assertTrue(pending.idempotencyKey != server.requests.last().first)
        } finally { gate.close() }
    }

    @Test fun committedLostReplyReplaysAcrossRestartThroughRealGate() = runComposeUiTest {
        val base = Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "audit-renewal-")
        val controllers = mutableListOf<TrustController>()
        try {
            val wallet = FakeWallet()
            val keys = DesktopKeyStore(dir, wallet)
            runBlocking { keys.csr() }
            val pair = KeyPair(keys.publicKey(), keys.privateKey())
            val caKey = TestCertificates.key()
            val ca = TestCertificates.cert(caKey)
            val old = TestCertificates.cert(pair, caKey, ca, ca = false, lifetimeDays = 15)
            val renewed = TestCertificates.cert(pair, caKey, ca, ca = false)
            val oldPem = pem(old)
            val renewedPem = pem(renewed)
            runBlocking { keys.save(DeviceTrust("fixture-device", oldPem, pem(ca), "fixture-session", "2030-01-01T00:00:00Z",
                listOf("https://localhost:8443"), old.notAfter.toInstant().toString())) }
            val requests = java.util.concurrent.CopyOnWriteArrayList<Pair<String?, String>>()
            val issued = java.util.concurrent.ConcurrentHashMap<Pair<String?, String>, String>()
            fun factory() = object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri): ApiTransport = error("Already enrolled")
                override suspend fun authenticated(trust: DeviceTrust): ApiTransport = KtorApiTransport(HttpClient(MockEngine { request ->
                    val body = when (request.url.encodedPath) {
                        "/v1/me" -> """{"member_id":"fixture-member"}"""
                        "/v1/endpoints" -> """{"endpoints":["https://localhost:8443"]}"""
                        else -> {
                            val identity = request.headers["Idempotency-Key"] to request.body.toByteArray().decodeToString()
                            requests += identity
                            val reply = issued.getOrPut(identity) { jsonBody("certificate" to renewedPem).toString() }
                            if (requests.size == 1) throw IOException("Reply lost after commit")
                            reply
                        }
                    }
                    respond(body, HttpStatusCode.OK, headersOf("Content-Type", "application/json"))
                }), trust.endpoints, HeaderPolicy(ClientInfo("0.1.6"), { "UTC" }, ::newTrustRequestKey), { false }, { it })
            }
            val first = TrustController(keys, factory(), "desktop", { Clock.System.now() }, false).also { controllers += it }
            runBlocking {
                first.restore()
                assertFailsWith<IOException> { first.refreshEndpointsAndRenew() }
                assertEquals(oldPem, keys.load()?.certificate)
            }
            first.close()
            val reopened = DesktopKeyStore(dir, wallet)
            val retry = TrustController(reopened, factory(), "desktop", { Clock.System.now() }, false).also { controllers += it }
            setContent { MaterialTheme { TrustGate(retry) { Text("Enrolled household") } } }
            waitUntil(timeoutMillis = 10_000) { runBlocking { reopened.load()?.certificate == renewedPem } }
            onNodeWithText("Enrolled household").assertExists()
            assertEquals(2, requests.size)
            assertTrue(requests[0] == requests[1], "Renewal retry must replay the durably stored request identity")
            assertEquals(1, issued.size, "Restart must not issue another certificate")
            runBlocking {
                val final = DesktopKeyStore(dir, wallet)
                assertEquals(renewedPem, final.load()?.certificate)
                assertNull(final.load()?.pendingRenewal)
                assertContentEquals(pair.public.encoded, final.publicKey().encoded)
            }
            val pixels = onRoot().captureToImage().toPixelMap()
            val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
            for (y in 0 until pixels.height) for (x in 0 until pixels.width) {
                val c = pixels[x, y]
                image.setRGB(x, y, ((c.alpha * 255).toInt() shl 24) or ((c.red * 255).toInt() shl 16) or ((c.green * 255).toInt() shl 8) or (c.blue * 255).toInt())
            }
            val evidence = File("build/test-evidence/audit-log-renewal").apply { mkdirs() }
            ImageIO.write(image, "png", File(evidence, "recovered.png"))
        } finally {
            controllers.forEach { it.close() }
            dir.toFile().deleteRecursively()
        }
    }
}
