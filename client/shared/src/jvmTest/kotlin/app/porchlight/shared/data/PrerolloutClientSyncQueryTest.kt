package app.porchlight.shared.data

import app.porchlight.shared.api.apis.ItemSyncApi
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.runBlocking
import kotlin.test.*

class PrerolloutClientSyncQueryTest {
    private val first = "00000000-0000-0000-0000-000000000001:2026-11-01T00:00:00+05:30:2026-11-08T00:00:00+05:30"
    private val second = "00000000-0000-0000-0000-000000000002:2026-11-01T00:00:00Z:2026-11-08T00:00:00Z"

    @Test fun oneAgendaWindowUsesExplodedFormAndEscapesPositiveOffset() = windows(listOf(first))
    @Test fun twoAgendaWindowsUseRepeatedKeysNotListToString() = windows(listOf(first, second))
    @Test fun emptyAgendaDoesNotSendAnEmptyArrayValue() = windows(emptyList())
    @Test fun continuationCursorsAlsoUseRepeatedIndependentValues() = windows(emptyList(), listOf("cursor+one:", "cursor/two="))

    private fun windows(windows: List<String>, cursors: List<String> = emptyList()) = runBlocking {
        var captured: Url? = null
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            captured = request.url
            respond("""{"type":"urn:porchlight:problem:not-found","status":404,"title":"Unavailable"}""",
                HttpStatusCode.NotFound, headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf("https://example.invalid"), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        try {
            val api = ItemSyncApi(GeneratedCall({ transport }, { "fixture-session" }))
            val include = "collaboration".takeIf { cursors.isEmpty() }
            assertFailsWith<ApiProblem> { api.itemSyncGet("00000000-0000-0000-0000-000000000003", "0.1.8", agenda = windows, agendaCursor = cursors, include = include) }
            val url = assertNotNull(captured)
            assertEquals(windows.takeIf { it.isNotEmpty() }, url.parameters.getAll("agenda"))
            assertEquals(cursors.takeIf { it.isNotEmpty() }, url.parameters.getAll("agenda_cursor"))
            assertEquals(include, url.parameters["include"])
            assertFalse(url.encodedQuery.contains("+"), "Positive offsets must be encoded as %2B, never form spaces")
            if (windows.isNotEmpty()) assertTrue(url.encodedQuery.contains("%3A"))
        } finally { transport.close() }
    }

    @Test fun scalarMapCallersKeepTheirEncoding() = runBlocking {
        var captured: Url? = null
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            captured = request.url
            respond("{}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf("https://example.invalid"), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        try {
            GeneratedCall({ transport }, { null }).request("GET", "/v1/search", mapOf("q" to "a+b & c", "limit" to "2"), emptyMap(), null)
            assertEquals("a+b & c", assertNotNull(captured).parameters["q"])
            assertEquals("2", captured.parameters["limit"])
        } finally { transport.close() }
    }
}
