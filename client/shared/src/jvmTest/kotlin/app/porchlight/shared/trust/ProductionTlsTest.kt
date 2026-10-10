package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.*
import java.io.IOException
import java.net.ConnectException
import java.net.InetAddress
import java.net.SocketTimeoutException
import java.nio.file.Files
import java.nio.file.Path
import java.security.KeyStore
import java.security.KeyPair
import java.security.KeyStoreException
import java.security.cert.X509Certificate
import java.time.format.DateTimeParseException
import java.util.concurrent.CopyOnWriteArrayList
import javax.crypto.AEADBadTagException
import javax.net.ssl.*
import kotlin.test.*
import kotlin.time.Clock
import kotlin.time.Instant
import io.ktor.client.network.sockets.ConnectTimeoutException

/** Real TLS 1.3 + real production OkHttp/Ktor transport, not a second verifier implementation. */
class ProductionTlsTest {
    @Test fun enrollmentPinAndHostnameFailBeforeAnySecretBytes() = runBlocking {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val key = TestCertificates.key()
        val leaf = TestCertificates.cert(key, caKey, ca, ca = false)
        val expired = TestCertificates.cert(key, caKey, ca, ca = false, expired = true)
        suspend fun probe(chain: Array<X509Certificate>, pin: String, host: String, success: Boolean) {
            LocalTlsServer(key, chain).use { server ->
                val factory = OkHttpTrustFactory(UnusedKeys(), ClientInfo("0.1.0"))
                val api = factory.enrollment(EnrollUri.parse("porchlight://enroll?host=$host:${server.port}&token=secret-token&ca=sha256:$pin"))
                try {
                    if (success) assertEquals("ok", api.request("POST", "/v1/enrollment/complete", jsonBody("token" to "secret-token", "password" to "secret-password"))["result"]?.jsonPrimitive?.content)
                    else assertFails { api.request("POST", "/v1/enrollment/complete", jsonBody("token" to "secret-token", "password" to "secret-password")) }
                } finally { api.close() }
                server.awaitIdle()
                assertEquals(if (success) 1 else 0, server.requests.size, "No HTTP bytes before trust and hostname checks")
            }
        }
        probe(arrayOf(leaf, ca), fingerprint(ca), "localhost", true)
        probe(arrayOf(leaf, ca), fingerprint(TestCertificates.cert(TestCertificates.key())), "localhost", false)
        probe(arrayOf(leaf), fingerprint(ca), "localhost", false)
        probe(arrayOf(leaf, ca), fingerprint(ca), "127.0.0.1", false) // DNS SAN is not an IP SAN
        probe(arrayOf(expired, ca), fingerprint(ca), "localhost", false)
        probe(arrayOf(leaf, ca), fingerprint(leaf), "localhost", false)
        val ipLeaf = TestCertificates.cert(key, caKey, ca, ca = false, host = "127.0.0.1")
        probe(arrayOf(ipLeaf, ca), fingerprint(ca), "127.0.0.1", true)
        probe(arrayOf(ipLeaf, ca), fingerprint(ca), "localhost", false)
    }

    @Test fun productionTransportNeverFollowsRedirects() = runBlocking {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val key = TestCertificates.key()
        val leaf = TestCertificates.cert(key, caKey, ca, ca = false)
        LocalTlsServer(key, arrayOf(leaf, ca)).use { server ->
            server.status = "307 Temporary Redirect"
            server.extraHeaders = "Location: ${server.url}/v1/redirected\r\n"
            val api = OkHttpTrustFactory(UnusedKeys(), ClientInfo("0.1.0")).enrollment(
                EnrollUri.parse("porchlight://enroll?host=localhost:${server.port}&token=token&ca=sha256:${fingerprint(ca)}"))
            try {
                val error = assertFailsWith<ApiProblem> { api.request("GET", "/v1/enrollment/complete") }
                assertEquals(307, error.status)
            } finally { api.close() }
            server.awaitIdle()
            assertEquals(1, server.requests.size)
            assertFalse(server.requests.single().contains("/v1/redirected"))
        }
    }

    @Test fun authenticatedTransportUsesOnlyHouseholdRoot() = runBlocking {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val serverKey = TestCertificates.key()
        val leaf = TestCertificates.cert(serverKey, caKey, ca, ca = false)
        val clientKey = TestCertificates.key()
        val clientLeaf = TestCertificates.cert(clientKey, caKey, ca, ca = false)
        LocalTlsServer(serverKey, arrayOf(leaf, ca), ca).use { server ->
            val keys = MemoryKeys(clientKey)
            for (root in listOf(ca, TestCertificates.cert(TestCertificates.key()))) {
                val trust = device(clientLeaf, root, server.url)
                val api = OkHttpTrustFactory(keys, ClientInfo("0.1.0")).authenticated(trust)
                try {
                    if (root == ca) api.request("GET", "/v1/me")
                    else assertFails { api.request("POST", "/v1/me", jsonBody("secret" to "must-not-send")) }
                } finally { api.close() }
            }
            server.awaitIdle()
            assertEquals(1, server.requests.size)
            assertEquals(clientLeaf.serialNumber, server.clientSerials.single())
        }
    }

    @Test fun realRenewalAtomicallyReplacesCertificateAndReusesSameKey() = runBlocking {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val serverKey = TestCertificates.key()
        val serverLeaf = TestCertificates.cert(serverKey, caKey, ca, ca = false)
        val base = Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "tls-renew-")
        try {
            val wallet = FakeWallet()
            val keys = DesktopKeyStore(dir, wallet)
            keys.csr()
            val pair = KeyPair(keys.publicKey(), keys.privateKey())
            val old = TestCertificates.cert(pair, caKey, ca, ca = false, lifetimeDays = 15)
            val renewed = TestCertificates.cert(pair, caKey, ca, ca = false)
            LocalTlsServer(serverKey, arrayOf(serverLeaf, ca), ca) { path ->
                when (path) {
                    "/v1/me" -> """{"member_id":"member"}"""
                    "/v1/endpoints" -> """{"endpoints":[]}""" // replaced below using server origin
                    else -> jsonBody("certificate" to pem(renewed), "not_after" to "deliberately-not-authoritative").toString()
                }
            }.use { server ->
                server.response = { path -> when (path) {
                    "/v1/me" -> """{"member_id":"member"}"""
                    "/v1/endpoints" -> """{"endpoints":["${server.url}"]}"""
                    else -> jsonBody("certificate" to pem(renewed), "not_after" to "deliberately-not-authoritative").toString()
                } }
                keys.save(device(old, ca, server.url))
                val controller = TrustController(keys, OkHttpTrustFactory(keys, ClientInfo("0.1.0")), "desktop", { Clock.System.now() }, false)
                controller.restore()
                controller.refreshEndpointsAndRenew()
                assertEquals(pem(renewed), keys.load()?.certificate)
                assertEquals(keys.certificateExpiry(pem(renewed)), keys.load()?.certificateExpiresAt)
                val reopened = DesktopKeyStore(dir, wallet)
                assertEquals(pem(renewed), reopened.load()?.certificate)
                assertContentEquals(pair.public.encoded, reopened.publicKey().encoded)
                controller.transport().request("GET", "/v1/me")
                controller.transport().close()
                val api = OkHttpTrustFactory(reopened, ClientInfo("0.1.0")).authenticated(checkNotNull(reopened.load()))
                try { api.request("GET", "/v1/me") } finally { api.close() }
                server.awaitIdle()
                assertEquals(listOf(old.serialNumber, old.serialNumber, old.serialNumber, renewed.serialNumber, renewed.serialNumber), server.clientSerials.toList())
                val body = WireJson.parseToJsonElement(server.requests.first { it.contains("/v1/devices/self/certificate") }.substringAfter("\r\n\r\n")).jsonObject
                val der = java.util.Base64.getMimeDecoder().decode(body.getValue("csr").jsonPrimitive.content.substringAfter("-----BEGIN CERTIFICATE REQUEST-----").substringBefore("-----END CERTIFICATE REQUEST-----"))
                val csr = org.bouncycastle.pkcs.PKCS10CertificationRequest(der)
                assertTrue(csr.isSignatureValid(org.bouncycastle.operator.jcajce.JcaContentVerifierProviderBuilder().build(pair.public)))
            }
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun productionFailureClassifiers() {
        assertTrue(isDefiniteConnectFailure(ConnectException()))
        assertTrue(isDefiniteConnectFailure(ConnectTimeoutException("connect timed out")))
        assertFalse(isDefiniteConnectFailure(SSLHandshakeException("untrusted")))
        assertFalse(isDefiniteConnectFailure(SocketTimeoutException("read timed out")))
        assertFalse(isDefiniteConnectFailure(ConnectException().apply { initCause(SSLHandshakeException("TLS")) }))
        for (alert in listOf("Received fatal alert: certificate_revoked", "handshake failed: CERTIFICATE_REVOKED")) {
            val error = IOException("wrapper", SSLException(alert))
            assertTrue(isCertificateRevoked(error))
            assertIs<DeviceCertificateRevoked>(classifyTlsFailure(error))
        }
        assertTrue(assertIs<DeviceCertificateRevoked>(classifyTlsFailure(SSLHandshakeException("Received fatal alert: certificate_revoked"))).requestNotSent)
        assertIs<RequestNotSent>(classifyTlsFailure(SSLHandshakeException("untrusted")))
        assertIs<RequestNotSent>(classifyTlsFailure(IOException("wrapper", SSLPeerUnverifiedException("wrong name"))))
        val readFailure = SocketTimeoutException("read timed out")
        assertSame(readFailure, classifyTlsFailure(readFailure))
        assertFalse(isCertificateRevoked(SSLException("certificate_expired")))
        assertFalse(isCertificateRevoked(IOException("CERTIFICATE_REVOKED")))
    }

    @Test fun corruptLocalRestoreOffersRecoveryOnBothPlatforms() = runBlocking {
        val errors = listOf(AEADBadTagException(), KeyStoreException(), IOException(), SerializationException("bad"),
            DateTimeParseException("bad", "bad", 0), LocalTrustUnavailable())
        for (platform in listOf("android", "desktop")) for (error in errors) {
            val keys = object : UnusedKeys() {
                var cleared = false
                override suspend fun load(): DeviceTrust? = throw error
                override suspend fun clear() { cleared = true }
            }
            val controller = TrustController(keys, OkHttpTrustFactory(keys, ClientInfo("0.1.0")), platform, { Clock.System.now() }, platform == "android")
            controller.restore()
            assertEquals(TrustState.LocalTrustLost(platform == "android"), controller.state.value)
            controller.startReEnrollment()
            assertTrue(keys.cleared)
            assertEquals(TrustState.Enrollment, controller.state.value)
        }
    }

    @Test fun nonIanaZonesAreMappedOrOmitted() {
        assertEquals("Europe/London", ianaZone("Europe/London"))
        assertEquals("Etc/GMT-2", ianaZone("GMT+02:00"))
        assertEquals("America/New_York", ianaZone("SystemV/EST5EDT"))
        assertNull(ianaZone("GMT+02:30"))
        assertNull(ianaZone("not-a-zone"))
        assertFalse(HeaderPolicy(ClientInfo("0.1.0"), { null }, { "key" }).headers("GET").containsKey("Porchlight-TZ"))
    }

    private fun device(leaf: X509Certificate, ca: X509Certificate, url: String) = DeviceTrust("device", pem(leaf), pem(ca), "session", "2035-01-01T00:00:00Z", listOf(url), leaf.notAfter.toInstant().toString())
}

internal open class UnusedKeys : JavaDeviceKeyStore {
    override suspend fun load(): DeviceTrust? = null
    override suspend fun csr(): String = error("Unused")
    override suspend fun save(trust: DeviceTrust, expectedPin: String?) = error("Unused")
    override suspend fun clear() {}
    override fun privateKey(): java.security.PrivateKey = error("Unused")
    override fun publicKey(): java.security.PublicKey = error("Unused")
}
private class MemoryKeys(private val pair: KeyPair) : UnusedKeys() {
    override fun privateKey() = pair.private
    override fun publicKey() = pair.public
}

/** Local loopback only. No dependency, external network, service or credential fixture. */
internal class LocalTlsServer(key: KeyPair, chain: Array<X509Certificate>, clientCa: X509Certificate? = null,
    var response: (String) -> String = { """{"result":"ok"}""" }) : AutoCloseable {
    var status = "200 OK"
    var extraHeaders = ""
    val requests = CopyOnWriteArrayList<String>()
    val clientSerials = CopyOnWriteArrayList<java.math.BigInteger>()
    private val context = SSLContext.getInstance("TLSv1.3").apply {
        val store = KeyStore.getInstance("PKCS12").apply { load(null); setKeyEntry("server", key.private, "test".toCharArray(), chain) }
        val managers = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm()).apply { init(store, "test".toCharArray()) }
        val trust = clientCa?.let { ca ->
            val roots = KeyStore.getInstance(KeyStore.getDefaultType()).apply { load(null); setCertificateEntry("client", ca) }
            TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply { init(roots) }.trustManagers
        }
        init(managers.keyManagers, trust, null)
    }
    private val socket = (context.serverSocketFactory.createServerSocket(0, 10, InetAddress.getByName("127.0.0.1")) as SSLServerSocket).apply {
        enabledProtocols = arrayOf("TLSv1.3"); needClientAuth = clientCa != null
    }
    val port get() = socket.localPort
    val url get() = "https://localhost:$port"
    @Volatile private var active = false
    private val thread = Thread {
        while (!socket.isClosed) {
            try {
                (socket.accept() as SSLSocket).use { client ->
                    active = true
                    client.soTimeout = 3000
                    client.startHandshake()
                    val input = client.inputStream
                    val header = StringBuilder()
                    while (!header.endsWith("\r\n\r\n")) {
                        val byte = input.read()
                        if (byte < 0) return@use
                        header.append(byte.toChar())
                    }
                    val size = Regex("(?i)Content-Length: (\\d+)").find(header)?.groupValues?.get(1)?.toInt() ?: 0
                    val body = input.readNBytes(size).toString(Charsets.UTF_8)
                    requests.add(header.toString() + body)
                    if (clientCa != null) clientSerials.add((client.session.peerCertificates[0] as X509Certificate).serialNumber)
                    val path = header.toString().substringBefore("\r\n").split(' ')[1]
                    val bytes = response(path).toByteArray()
                    client.outputStream.write("HTTP/1.1 $status\r\n${extraHeaders}Content-Type: application/json\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray() + bytes)
                    client.outputStream.flush()
                }
            } catch (_: IOException) { /* Trust/hostname failures close without any HTTP bytes. */ }
            finally { active = false }
        }
    }.apply { isDaemon = true; start() }
    fun awaitIdle() { repeat(100) { if (!active) return; Thread.sleep(10) }; check(!active) { "Local TLS server did not finish" } }
    override fun close() { socket.close(); thread.join(4000); check(!thread.isAlive) { "Local TLS server did not stop" } }
}
