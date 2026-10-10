package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

/**
 * fbcc c3a-rr1 OOS: a reply the client cannot read as a server decision (proxy 5xx page, server 5xx that rolled
 * back, undecodable 2xx) does not seal the write. The screen keeps the frozen key and immutable request so the only
 * way forward is an identical retry (or an explicit abandon); a resubmission can never duplicate a committed create.
 */
class AmbiguousOutcomeRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    private enum class FirstReply(val commits: Boolean) {
        ProxyBadGateway(true), ProxyGatewayTimeoutAfterCommit(true), ServerProblem500(false),
        UndecodableSuccessPage(true), UndecodableSuccessJson(true),
    }

    private fun MockRequestHandleScope.first(reply: FirstReply): io.ktor.client.request.HttpResponseData = when (reply) {
        FirstReply.ProxyBadGateway -> respond("<html>502 Bad Gateway</html>", HttpStatusCode.BadGateway, headersOf(HttpHeaders.ContentType, "text/html"))
        FirstReply.ProxyGatewayTimeoutAfterCommit -> respond("upstream timed out", HttpStatusCode.GatewayTimeout, headersOf(HttpHeaders.ContentType, "text/plain"))
        FirstReply.ServerProblem500 -> respond("""{"type":"urn:porchlight:problem:internal","status":500,"title":"Internal error"}""",
            HttpStatusCode.InternalServerError, headersOf(HttpHeaders.ContentType, "application/problem+json"))
        FirstReply.UndecodableSuccessPage -> respond("<html>captive portal</html>", HttpStatusCode.OK, headersOf(HttpHeaders.ContentType, "text/html"))
        FirstReply.UndecodableSuccessJson -> respond("""{"unexpected":true}""", HttpStatusCode.Created, headersOf(HttpHeaders.ContentType, "application/json"))
    }

    @Test fun ambiguousRepliesKeepTheFrozenKeyAndIdenticalRetryNeverDuplicatesACreate() = runTest {
        for (reply in FirstReply.entries) {
            val cache = memoryCache { now }
            // Emulates the server's idempotency table: one commit per key, replayed for the same key.
            val committed = mutableMapOf<String, String>()
            val keys = mutableListOf<String?>()
            val bodies = mutableListOf<String>()
            var attempt = 0
            val api = KtorApiTransport(HttpClient(MockEngine { req ->
                assertEquals(HttpMethod.Post, req.method)
                val key = req.headers["Idempotency-Key"]!!
                keys += key; bodies += req.body.toByteArray().decodeToString()
                val stored = committed[key]
                val isFirst = ++attempt == 1
                if (isFirst && !reply.commits) return@MockEngine first(reply)
                val body = stored ?: WireJson.encodeToString(ItemResponse.serializer(), itemResponse("created-${committed.size + 1}")).also { committed[key] = it }
                if (isFirst) first(reply) else respond(body, HttpStatusCode.Created, headersOf(HttpHeaders.ContentType, "application/json"))
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
                data.items.afterWrite = {}
                val body = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("one")))
                var generated = 0
                val screen = ScreenWrites { "intent-${++generated}" }
                screen.submit { data.items.create(body, it).feedback() }
                val ambiguous = screen.state.value
                assertFalse(ambiguous.saved, "$reply")
                assertEquals("intent-1", ambiguous.retryKey, "$reply: the frozen key must survive (${ambiguous.error})")
                assertTrue(ambiguous.fieldErrors.isEmpty(), "$reply")
                // A new action cannot replace the pending request with a fresh key.
                screen.submit { data.items.create(body, it).feedback() }
                assertEquals("intent-1", screen.state.value.retryKey, "$reply")
                assertEquals(1, keys.size, "$reply: blocked resubmission must not reach the server")
                screen.retry()
                assertTrue(screen.state.value.saved, "$reply: ${screen.state.value.error}")
                assertNull(screen.state.value.retryKey, "$reply")
                assertEquals(listOf<String?>("intent-1", "intent-1"), keys, "$reply")
                assertEquals(1, bodies.toSet().size, "$reply: retry must replay the identical body")
                assertEquals(1, committed.size, "$reply: exactly one create committed")
                assertEquals(1, generated, "$reply: no replacement key was generated")
            } finally { api.close(); cache.close() }
        }
    }

    @Test fun sealedBusinessRejectionStillReleasesTheKeyForACorrectedWrite() = runTest {
        val cache = memoryCache { now }
        val keys = mutableListOf<String?>()
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            keys += req.headers["Idempotency-Key"]
            respond("""{"type":"urn:porchlight:problem:validation","status":422,"title":"Invalid","errors":[{"field":"title","code":"required","message":"Required"}]}""",
                HttpStatusCode.UnprocessableEntity, headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            val body = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("one")))
            var generated = 0
            val screen = ScreenWrites { "intent-${++generated}" }
            screen.submit { data.items.create(body, it).feedback() }
            assertNull(screen.state.value.retryKey)
            assertEquals(mapOf("title" to "Required"), screen.state.value.fieldErrors)
            screen.submit { data.items.create(body, it).feedback() }
            assertEquals(listOf<String?>("intent-1", "intent-2"), keys)
        } finally { api.close(); cache.close() }
    }

    @Test fun ambiguityPredicateSeparatesServerDecisionsFromUnreadableReplies() {
        assertTrue(ApiProblem("http-error", 502, "Request failed").isAmbiguousOutcome())
        assertTrue(ApiProblem("http-error", 404, "Request failed").isAmbiguousOutcome()) // not a problem document
        assertTrue(ApiProblem("invalid-response", 502, "Invalid API response").isAmbiguousOutcome())
        assertTrue(ApiProblem("internal", 500, "Internal error").isAmbiguousOutcome())
        assertTrue(ApiProblem("unavailable", 503, "Unavailable").isAmbiguousOutcome())
        assertFalse(ApiProblem("validation", 422, "Invalid").isAmbiguousOutcome())
        assertFalse(ApiProblem("version-conflict", 409, "Changed").isAmbiguousOutcome())
        assertFalse(ApiProblem("forbidden", 403, "Forbidden").isAmbiguousOutcome())
        assertFalse(ApiProblem("idempotency-in-progress", 409, "Busy").isAmbiguousOutcome())
    }
}
