package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class TransportTest {
    @Test fun sendsHeadersAndKeepsMutationIdentityAcrossConnectFailover() = runTest {
        var attempts = 0; var randomCalls = 0
        val keys = mutableListOf<String?>(); val bodies = mutableListOf<String>()
        val mock = MockEngine { request ->
            attempts++
            assertEquals("0.1.0", request.headers["Porchlight-Client-Version"])
            assertEquals("Europe/Helsinki", request.headers["Porchlight-TZ"])
            assertEquals("Bearer session", request.headers["Authorization"])
            assertEquals("3", request.headers["If-Match"])
            keys += request.headers["Idempotency-Key"]; bodies += request.body.toByteArray().decodeToString()
            if (attempts == 1) throw ConnectFailure()
            respond("{}", HttpStatusCode.OK, headersOf("Content-Type", "application/json"))
        }
        val api = KtorApiTransport(HttpClient(mock), listOf("https://lan:8443", "https://funnel"), HeaderPolicy(ClientInfo("0.1.0"), { "Europe/Helsinki" }, { randomCalls++; "key" }), { it is ConnectFailure })
        try {
            api.request("POST", "/v1/auth/password", jsonBody("current_password" to "password"), "session", version = 3)
            assertEquals(2, attempts); assertEquals(1, randomCalls); assertEquals(keys[0], keys[1]); assertEquals(bodies[0], bodies[1])
        } finally { api.close() }
    }
    @Test fun lanEndpointsComeBeforeFunnelAndRejectNonOrigins() {
        assertEquals(listOf("https://porchlight.lan:8443", "https://10.0.2.2:8443", "https://node.tail.ts.net"),
            orderedEndpoints(listOf("https://node.tail.ts.net", "https://porchlight.lan:8443", "https://10.0.2.2:8443")))
        for (bad in listOf("http://lan", "https://user:secret@lan", "https://lan/path", "https://lan?token=secret", "https://lan#fragment"))
            assertFails { orderedEndpoints(listOf(bad)) }
    }
    @Test fun httpAndTlsErrorsNeverFailOver() = runTest {
        var attempts = 0
        val engine = MockEngine {
            attempts++
            respond("""{"type":"urn:porchlight:problem:session-invalid","title":"Invalid","status":401}""", HttpStatusCode.Unauthorized, headersOf("Content-Type", "application/problem+json; charset=utf-8"))
        }
        val api = KtorApiTransport(HttpClient(engine), listOf("https://lan", "https://funnel"), HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        try { assertEquals("session-invalid", assertFailsWith<ApiProblem> { api.request("GET", "/v1/me") }.type); assertEquals(1, attempts) }
        finally { api.close() }
        var tlsAttempts = 0
        val tls = KtorApiTransport(HttpClient(MockEngine { tlsAttempts++; throw IllegalStateException("TLS validation failed") }), listOf("https://lan", "https://funnel"), HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        try { assertFailsWith<IllegalStateException> { tls.request("GET", "/v1/me") }; assertEquals(1, tlsAttempts) } finally { tls.close() }
    }
}
