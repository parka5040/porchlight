package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient3bScopeTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun row(name: String) = WireJson.decodeFromString<ListResponse>(checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() })

    @Test fun scoped404ProbesParentAndEvictsAllHiddenScopes() = runTest {
        val cache = memoryCache { now }
        val requested = mutableListOf<String?>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            requested += req.url.parameters["occurrence_id"]
            respond("Not found", HttpStatusCode.NotFound)
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        try {
            val scoped = row("list-occurrence-check")
            cache.putList(row("list-occurrence-template")); cache.putList(scoped); cache.putList(row("list-occurrence-other"))
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            assertEquals(404, assertFailsWith<ApiProblem> { repo.get(scoped.id, scoped.occurrenceId) }.status)
            assertTrue(cache.view.value.lists.isEmpty(), "A hidden parent must evict its template and every occurrence scope")
            assertEquals(listOf(scoped.occurrenceId, null), requested)
        } finally { cache.close(); transport.close() }
    }

    @Test fun activeScopeSurvivesHistoryChurnButNotVisibilityLoss() = runTest {
        val cache = memoryCache { now }
        val active = row("list-occurrence-check")
        try {
            cache.putList(active)
            cache.keepListScope(active.id, active.occurrenceId) {
                repeat(40) { cache.putList(active.copy(occurrenceId = "scope-$it", propertyEntries = emptyList())) }
                assertTrue(cache.view.value.lists.any { it.occurrenceId == active.occurrenceId }, "Visible editor scope was pruned")
                assertEquals(32, cache.view.value.lists.size)
                cache.evictList(active.id)
                assertTrue(cache.view.value.lists.isEmpty(), "Pinning must never override visibility eviction")
            }
            cache.putList(active)
            cache.keepListScope(active.id, active.occurrenceId) { }
            repeat(40) { cache.putList(active.copy(occurrenceId = "scope-$it", propertyEntries = emptyList())) }
            assertTrue(cache.view.value.lists.none { it.occurrenceId == active.occurrenceId }, "Closed editor must release its pin")
        } finally { cache.close() }
    }

}
