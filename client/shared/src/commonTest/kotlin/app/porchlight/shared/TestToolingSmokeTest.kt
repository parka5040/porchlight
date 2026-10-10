package app.porchlight.shared

import app.cash.turbine.test
import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.engine.mock.MockEngine
import io.ktor.client.engine.mock.respond
import io.ktor.client.plugins.contentnegotiation.ContentNegotiation
import io.ktor.client.request.get
import io.ktor.http.ContentType
import io.ktor.http.HttpHeaders
import io.ktor.http.headersOf
import io.ktor.serialization.kotlinx.json.json
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.Serializable
import kotlin.test.Test
import kotlin.test.assertEquals

/** Keeps the shared test tooling (coroutines-test, Turbine, Ktor mock engine + JSON) compiling and running. */
class TestToolingSmokeTest {
    @Serializable
    private data class Health(val status: String)

    @Test
    fun turbineObservesAFlow() = runTest {
        flowOf("Porchlight").test {
            assertEquals("Porchlight", awaitItem())
            awaitComplete()
        }
    }

    @Test
    fun mockEngineServesJson() = runTest {
        val engine = MockEngine {
            respond("""{"status":"ok"}""", headers = headersOf(HttpHeaders.ContentType, ContentType.Application.Json.toString()))
        }
        val client = HttpClient(engine) { install(ContentNegotiation) { json() } }
        assertEquals(Health("ok"), client.get("https://porchlight.test/v1/health").body<Health>())
    }
}
