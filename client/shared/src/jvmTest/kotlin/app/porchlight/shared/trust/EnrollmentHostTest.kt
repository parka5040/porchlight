package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.runBlocking
import java.io.IOException
import java.net.UnknownHostException
import java.nio.channels.UnresolvedAddressException
import java.util.concurrent.CancellationException
import javax.net.ssl.SSLHandshakeException
import kotlin.test.*

/** Android and desktop share tlsMain's OkHttp trust factory and failure classifier. */
class EnrollmentHostTest {
    @Test fun lanIpLiteralEnrollmentTarget() {
        val uri = EnrollUri.parse("porchlight://enroll?host=192.168.77.50:8444&token=fixture&ca=sha256:${"ab".repeat(32)}")
        assertEquals("192.168.77.50:8444", uri.host)
        assertEquals("https://192.168.77.50:8444", EnrollmentTarget.parse(uri.host).baseUrl)
    }

    @Test fun ipSanAcceptsIpLiteralWithPinnedHouseholdCa() = runBlocking {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val key = TestCertificates.key()
        val leaf = TestCertificates.cert(key, caKey, ca, ca = false, host = "127.0.0.1")
        LocalTlsServer(key, arrayOf(leaf, ca)).use { server ->
            val uri = EnrollUri.parse("porchlight://enroll?host=127.0.0.1:${server.port}&token=fixture&ca=sha256:${fingerprint(ca)}")
            val api = OkHttpTrustFactory(UnusedKeys(), ClientInfo("0.1.0")).enrollment(uri)
            try { assertEquals("ok", api.request("POST", "/v1/enrollment/complete", jsonBody("token" to "fixture"))["result"]?.toString()?.trim('"')) }
            finally { api.close() }
            assertEquals(1, server.requests.size)
        }
    }

    @Test fun unknownHostIncludingKtorWrapperIsConnectFailure() {
        for (error in listOf(UnknownHostException(), IOException("Ktor wrapper", UnknownHostException()))) {
            assertTrue(isDefiniteConnectFailure(error))
        }
        assertFalse(isDefiniteConnectFailure(IOException("TLS wrapper", SSLHandshakeException("wrong SAN").apply { initCause(UnknownHostException()) })))
        assertFalse(isDefiniteConnectFailure(CancellationException().apply { initCause(UnknownHostException()) }))
    }

    @Test fun unresolvedAddressIncludingWrapperIsConnectFailure() {
        assertTrue(isDefiniteConnectFailure(UnresolvedAddressException()))
        assertTrue(isDefiniteConnectFailure(IOException("Ktor wrapper", UnresolvedAddressException())))
        assertFalse(isDefiniteConnectFailure(SSLHandshakeException("wrong SAN").apply { initCause(UnresolvedAddressException()) }))
        assertFalse(isDefiniteConnectFailure(CancellationException().apply { initCause(UnresolvedAddressException()) }))
    }

    @Test fun nameResolutionFailureFallsThroughToFunnelWithoutChangingMutation() = runBlocking {
        for (error in listOf(UnknownHostException(), IOException("Ktor wrapper", UnknownHostException()),
            UnresolvedAddressException(), IOException("Ktor wrapper", UnresolvedAddressException()))) {
            val hosts = mutableListOf<String>()
            val identities = mutableListOf<String?>()
            val bodies = mutableListOf<String>()
            val engine = MockEngine { request ->
                hosts += request.url.host
                identities += request.headers["Idempotency-Key"]
                bodies += request.body.toByteArray().decodeToString()
                if (request.url.host == "porchlight.lan") throw error
                respond("{}", HttpStatusCode.OK, headersOf("Content-Type", "application/json"))
            }
            val api = KtorApiTransport(HttpClient(engine), listOf("https://quiet-node.tail123.ts.net", "https://porchlight.lan:8443"),
                HeaderPolicy(ClientInfo("0.1.0"), { "UTC" }, { "fixed-key" }), ::isDefiniteConnectFailure, ::classifyTlsFailure)
            try {
                api.request("POST", "/v1/items", jsonBody("title" to "fixture"))
                assertEquals(listOf("porchlight.lan", "quiet-node.tail123.ts.net"), hosts)
                assertEquals(listOf<String?>("fixed-key", "fixed-key"), identities)
                assertEquals(bodies[0], bodies[1])
            } finally { api.close() }
        }
    }
}
