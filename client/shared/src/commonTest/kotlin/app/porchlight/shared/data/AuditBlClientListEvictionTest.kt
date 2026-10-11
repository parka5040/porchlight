package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClientListEvictionTest {
    private val collabNow = kotlin.time.Instant.parse("2026-10-06T12:00:00Z")
    @Test fun batchTreatsVisibilityDeniedAsSuccessfulEvictionAndContinues() = runTest {
        val cache = memoryCache { collabNow }
        val first = listFixture()
        val second = first.copy(id = collabId("two"), propertyEntries = emptyList())
        val requested = mutableListOf<String>()
        val transport = KtorApiTransport(HttpClient(MockEngine {
            requested += it.url.encodedPath
            respond("""{"type":"urn:porchlight:problem:forbidden","status":403,"title":"Unavailable"}""",
                HttpStatusCode.Forbidden, headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { collabNow })
        try {
            cache.install(emptySnapshot(1).copy(lists = listOf(first, second)))
            data.lists.refetchByIds(linkedSetOf(first.id, second.id))
            assertEquals(listOf("/v1/lists/${first.id}", "/v1/lists/${second.id}"), requested)
            assertTrue(cache.view.value.lists.isEmpty())
        } finally { data.close(); transport.close() }
    }
}
