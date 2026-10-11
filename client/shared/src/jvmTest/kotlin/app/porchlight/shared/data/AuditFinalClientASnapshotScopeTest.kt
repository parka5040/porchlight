package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientASnapshotScopeTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun row(name: String) = WireJson.decodeFromString<ListResponse>(checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() })

    @Test fun snapshotThenScoped403EvictsHiddenParent() = deniedScope(403, true)
    @Test fun snapshotThenScoped404ProbesAndEvictsHiddenParent() = deniedScope(404, true)
    @Test fun snapshotThenMissingOccurrenceFencesOlderScopedGet() = deniedScope(404, false)
    @Test fun snapshotThenInvalidOccurrenceFencesOlderScopedGet() = deniedScope(422, false)

    private fun deniedScope(status: Int, hidden: Boolean) = runTest {
        val cache = memoryCache { now }
        val template = row("list-occurrence-template")
        val scoped = row("list-occurrence-check")
        val snapshot = emptySnapshot(2).copy(lists = listOf(template), items = listOf(itemRead(template.itemId!!)),
            occurrences = listOf(occurrenceRead(scoped.occurrenceId!!, template.itemId)))
        val oldStarted = CompletableDeferred<Unit>(); val releaseOld = CompletableDeferred<Unit>()
        var scopedRequests = 0; var probes = 0; var snapshotReceived = false
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            when (request.url.encodedPath) {
                "/v1/snapshot" -> { snapshotReceived = true; respond(WireJson.encodeToString(snapshot), headers = headersOf(HttpHeaders.ContentType, "application/json")) }
                "/v1/agenda", "/v1/board" -> respond("{\"items\":[]}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                else -> if (request.url.parameters["occurrence_id"] != null) {
                    if (scopedRequests++ == 0) {
                        oldStarted.complete(Unit); releaseOld.await()
                        respond(WireJson.encodeToString(scoped), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                    } else {
                        assertTrue(snapshotReceived, "Visibility changes after the readable snapshot")
                        respond(if (status == 422) """{"type":"urn:porchlight:problem:validation-failed","title":"Invalid occurrence","status":422,"errors":[{"field":"occurrence_id","code":"invalid-reference"}]}""" else """{"type":"urn:porchlight:problem:${if (status == 403) "forbidden" else "not-found"}","title":"Denied","status":$status}""",
                            HttpStatusCode.fromValue(status), headersOf(HttpHeaders.ContentType, "application/problem+json"))
                    }
                } else {
                    probes++
                    if (hidden) respond("Denied", HttpStatusCode.NotFound)
                    else respond(WireJson.encodeToString(template), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                }
            }
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.putList(template); cache.putList(scoped)
            val older = async { runCatching { data.lists.get(scoped.id, scoped.occurrenceId) } }
            oldStarted.await()
            data.sync.refresh()
            if (hidden) assertTrue(cache.view.value.lists.isEmpty(), "A later parent denial must suppress the earlier snapshot template")
            else {
                assertEquals(listOf(template), cache.view.value.lists)
                releaseOld.complete(Unit)
                assertTrue(older.await().isFailure, "A missing occurrence must fence an older concurrent scoped GET")
                assertEquals(listOf(template), cache.view.value.lists)
            }
            assertEquals(if (status == 404) 1 else 0, probes)
            releaseOld.complete(Unit)
            if (hidden) { assertTrue(older.await().isFailure); assertTrue(cache.view.value.lists.isEmpty()) }
            assertEquals(2, cache.sync.value?.cursor)
        } finally { releaseOld.complete(Unit); transport.close(); cache.close() }
    }
}
