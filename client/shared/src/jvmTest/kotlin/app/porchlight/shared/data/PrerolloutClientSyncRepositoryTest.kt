package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.encodeToString
import kotlin.test.*
import kotlin.time.Duration.Companion.minutes

class PrerolloutClientSyncRepositoryTest {
    private val f = SyncBundleFixtures
    @Test fun followsEveryAgendaAndLongThreadContinuationAndInstallsEquivalentFullScopes() = runTest {
        val snapshot = f.snapshot
        val item = snapshot.items.single()
        val member = snapshot.memberSettings.memberId
        val other = "00000000-0000-7000-8000-000000000099"
        val windows = listOf(member, other).map { ItemAgendaWindow(it, f.from, f.to) }
        val comments = (1..451).map { snapshot.comments.single().copy(id = "00000000-0000-7000-8000-%012d".format(it)) }
        fun agenda(who: String, index: Int) = item.copy(agenda = f.agenda.agenda!!.copy(memberId = who,
            occurrenceId = "00000000-0000-7000-8001-%012d".format(index), dueAt = f.now + index.minutes, startsAt = null, endsAt = null))
        val a = (1..205).map { agenda(member, it) }; val b = (1..205).map { agenda(other, it) }
        fun scope(who: String, input: String?, next: String?) = AgendaCoverage(who, f.from, f.to, next == null, cursor = input, nextCursor = next)
        fun rows(who: String, list: List<ItemRead>) = SyncAgenda(who, f.from, f.to, list)
        val first = f.bundle().let { p -> p.copy(comments = comments.take(200), commentsNextCursor = "comments-200",
            coverage = p.coverage.copy(commentsComplete = false, agenda = listOf(scope(member, null, "a-200"), scope(other, null, "b-empty"))),
            agenda = listOf(rows(member, a.take(200)), rows(other, emptyList()))) }
        val second = f.continuation(first, listOf(rows(member, a.drop(200)), rows(other, b.take(195))),
            listOf(scope(member, "a-200", null), scope(other, "b-empty", "b-195")), comments.drop(200).take(200), "comments-200", "comments-400")
        val third = f.continuation(first, listOf(rows(other, b.drop(195))), listOf(scope(other, "b-195", null)), comments.drop(400), "comments-400")
        val pages = listOf(first, second, third)
        val urls = mutableListOf<Url>()
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            urls += request.url
            respond(WireJson.encodeToString(pages[urls.lastIndex]), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        val cache = memoryCache { f.now }; val full = memoryCache { f.now }
        try {
            val repository = ItemSyncRepository(GeneratedCall({ transport }, { "fixture-session" }), { true })
            val result = assertIs<ItemSyncResult.Bundle>(repository.fetch(item.id, windows, true))
            assertEquals(451, result.rows.comments.size)
            assertEquals(pages.map { it.coverage }, result.coverage.pages)
            assertEquals(3, urls.size)
            assertEquals(2, urls.first().parameters.getAll("agenda")?.size)
            assertEquals("collaboration", urls.first().parameters["include"])
            assertEquals(listOf("a-200", "b-empty"), urls[1].parameters.getAll("agenda_cursor"))
            assertEquals(listOf("b-195"), urls[2].parameters.getAll("agenda_cursor"))
            urls.drop(1).forEach { assertNull(it.parameters["agenda"]); assertNull(it.parameters["include"]) }
            val omitted = listOf("000000000000", "00000000020a", "000000009999").map { suffix ->
                comments.first().copy(id = "00000000-0000-7000-8000-$suffix")
            }
            cache.install(snapshot.copy(comments = (comments + omitted).sortedBy { it.id }), projections = Projections(emptyList(), AgendaWindow(f.from, f.to, member, emptyList())))
            cache.inSession { cache.installItemBundle(it, result.coverage, result.rows) }
            full.install(snapshot.copy(comments = comments), projections = Projections(listOf(f.board), AgendaWindow(f.from, f.to, member, a + b)))
            assertEquals(full.view.value.items, cache.view.value.items)
            assertEquals(full.view.value.occurrences, cache.view.value.occurrences)
            assertEquals(full.view.value.comments.sortedBy { it.id }, cache.view.value.comments.sortedBy { it.id })
            assertEquals(full.view.value.agenda.sortedBy { it.agenda!!.memberId + it.agenda.occurrenceId }, cache.view.value.agenda.sortedBy { it.agenda!!.memberId + it.agenda.occurrenceId })
            assertTrue(cache.view.value.commentHistory.getValue(item.id).complete)
        } finally { cache.close(); full.close(); transport.close() }
    }

    @Test fun absentCapabilityNeverCallsUnsupportedEndpointOrEvicts() = runTest {
        var calls = 0
        val transport = problemTransport(404, "not-found") { calls++ }
        try {
            assertEquals(ItemSyncResult.Fallback, ItemSyncRepository(GeneratedCall({ transport }, { null }), { false }).fetch("item"))
            assertEquals(0, calls)
        } finally { transport.close() }
    }

    @Test fun capabilityWithdrawnDuringRequestCannotTurnUnsupported404IntoDeletion() = runTest {
        var supported = true
        val transport = problemTransport(404, "not-found") { supported = false }
        try {
            assertEquals(ItemSyncResult.Fallback, ItemSyncRepository(GeneratedCall({ transport }, { null }), { supported }).fetch("item"))
        } finally { transport.close() }
    }

    @Test fun capable404IsEvictionButUnsupportedEndpointAndOnlyOversized422AreFallbacks() = runTest {
        for ((status, type, expected) in listOf(
            Triple(404, "not-found", ItemSyncResult.Evicted), Triple(404, "http-error", ItemSyncResult.Fallback),
            Triple(405, "http-error", ItemSyncResult.Fallback), Triple(501, "http-error", ItemSyncResult.Fallback),
            Triple(422, "item-sync-too-large", ItemSyncResult.Fallback))) {
            val transport = problemTransport(status, type)
            try { assertEquals(expected, ItemSyncRepository(GeneratedCall({ transport }, { null }), { true }).fetch("item")) }
            finally { transport.close() }
        }
        val transport = problemTransport(422, "validation-failed")
        try { assertFailsWith<ApiProblem> { ItemSyncRepository(GeneratedCall({ transport }, { null }), { true }).fetch("item") } }
        finally { transport.close() }
    }

    @Test fun failedContinuationDoesNotReturnAnyPartialReplacementCoverage() = runTest {
        var count = 0
        val p = f.bundle().let { it.copy(coverage = it.coverage.copy(commentsComplete = false), commentsNextCursor = "tail") }
        val transport = KtorApiTransport(HttpClient(MockEngine {
            if (count++ == 0) respond(WireJson.encodeToString(p), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            else respond("""{"type":"urn:porchlight:problem:unavailable","status":503,"title":"Unavailable"}""", HttpStatusCode.ServiceUnavailable,
                headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        try {
            assertFailsWith<ApiProblem> { ItemSyncRepository(GeneratedCall({ transport }, { null }), { true }).fetch(p.coverage.itemId,
                listOf(ItemAgendaWindow(p.coverage.agenda.single().memberId, f.from, f.to)), true) }
            assertEquals(2, count)
        } finally { transport.close() }
    }

    private fun problemTransport(status: Int, type: String, called: () -> Unit = {}) = KtorApiTransport(HttpClient(MockEngine {
        called()
        respond("""{"type":"urn:porchlight:problem:$type","status":$status,"title":"Unavailable"}""", HttpStatusCode.fromValue(status),
            headersOf(HttpHeaders.ContentType, "application/problem+json"))
    }), listOf(testOrigin()), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
}
