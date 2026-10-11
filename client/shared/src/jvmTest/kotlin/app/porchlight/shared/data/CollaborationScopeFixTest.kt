package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.HttpResponseData
import io.ktor.http.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class CollaborationScopeFixTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun recording(name: String) = checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() }
    private fun row(name: String) = WireJson.decodeFromString<ListResponse>(recording(name))
    private fun transport(handler: MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine { handler(it) }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
    private fun repo(transport: ApiTransport, cache: ReadCache) = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())

    @Test fun realInvalidReferenceAnd404EvictOnlyTheirScopeAndBatchContinues() = runTest {
        for (status in listOf(HttpStatusCode.UnprocessableEntity, HttpStatusCode.NotFound)) {
            val stale = row("list-occurrence-check")
            val live = row("list-occurrence-other")
            val template = row("list-occurrence-template")
            val unrelated = listFixture().copy(propertyEntries = emptyList())
            val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
            PorchlightDb.Schema.create(driver)
            val q = PorchlightDb(driver).cacheQueries
            val cache = ReadCache(driver) { now }
            val queries = mutableListOf<Pair<String, String?>>()
            val transport = transport { req ->
                val scope = req.url.parameters["occurrence_id"]
                queries += req.url.encodedPath.substringAfterLast('/') to scope
                if (scope == stale.occurrenceId) respond(if (status.value == 422) recording("list-occurrence-invalid-reference") else "Not found", status,
                    headersOf(HttpHeaders.ContentType, if (status.value == 422) "application/problem+json" else "text/plain"))
                else respond(WireJson.encodeToString(ListResponse.serializer(), when {
                    req.url.encodedPath.endsWith(unrelated.id) -> unrelated.copy(version = 2)
                    scope == live.occurrenceId -> live.copy(version = 2)
                    else -> template.copy(version = 2)
                }), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
            try {
                val mark = cache.receiptMark()
                cache.putList(stale); cache.putList(live); cache.putList(template); cache.putList(unrelated)
                repo(transport, cache).refetchByIds(linkedSetOf(stale.id, unrelated.id))
                assertEquals(if (status.value == 404) 5 else 4, queries.size) // 404 probes template visibility.
                assertEquals(setOf(null, live.occurrenceId), cache.view.value.lists.filter { it.id == stale.id }.map { it.occurrenceId }.toSet())
                assertTrue(cache.view.value.lists.all { it.version == 2 })
                assertTrue(q.allEntrys().executeAsList().none { it.occurrence_id == stale.occurrenceId })
                queries.clear()
                repo(transport, cache).refetchByIds(linkedSetOf(stale.id, unrelated.id))
                assertEquals(3, queries.size) // A second tickle never retries the dead scope.
                // Even an older in-flight snapshot must not reapply the evicted scoped receipts.
                cache.installStaged(emptySnapshot(9).copy(items = listOf(itemRead(stale.itemId!!)),
                    occurrences = listOf(occurrenceRead(stale.occurrenceId!!, stale.itemId), occurrenceRead(live.occurrenceId!!, live.itemId!!)),
                    lists = listOf(template, unrelated)), receivedAfter = mark)
                assertTrue(cache.view.value.lists.none { it.occurrenceId == stale.occurrenceId })
            } finally { cache.close(); transport.close() }
        }
    }

    @Test fun unrelatedValidationAndTransientFailuresRemainCachedButDoNotAbortOtherIds() = runTest {
        val stale = row("list-occurrence-check")
        val live = row("list-occurrence-other")
        val unrelated = listFixture().copy(propertyEntries = emptyList())
        val invalidField = recording("list-occurrence-invalid-reference").replace("occurrence_id", "other_field")
        val cache = memoryCache { now }
        val queries = mutableListOf<String>()
        var failure = HttpStatusCode.UnprocessableEntity
        val transport = transport { req ->
            queries += req.url.encodedPath
            if (req.url.parameters["occurrence_id"] == stale.occurrenceId) respond(invalidField, failure, headersOf(HttpHeaders.ContentType, if (failure.value == 422) "application/problem+json" else "text/plain"))
            else respond(WireJson.encodeToString(ListResponse.serializer(), if (req.url.encodedPath.endsWith(unrelated.id)) unrelated.copy(version = 2) else live.copy(version = 2)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            cache.putList(stale); cache.putList(live); cache.putList(unrelated)
            for (status in listOf(HttpStatusCode.UnprocessableEntity, HttpStatusCode.ServiceUnavailable)) {
                failure = status; queries.clear()
                assertEquals(status.value, assertFailsWith<ApiProblem> { repo(transport, cache).refetchByIds(linkedSetOf(stale.id, unrelated.id)) }.status)
                assertEquals(3, queries.size)
                assertTrue(cache.view.value.lists.any { it.occurrenceId == stale.occurrenceId })
                assertEquals(2, cache.view.value.lists.single { it.id == unrelated.id }.version)
                assertEquals(2, cache.view.value.lists.single { it.occurrenceId == live.occurrenceId }.version)
            }
        } finally { cache.close(); transport.close() }
    }

    @Test fun scopeRetentionAndTickleGetsAreBoundedAndRemovedScopesCannotReplay() = runTest {
        val base = row("list-occurrence-check")
        val itemId = requireNotNull(base.itemId)
        val template = row("list-occurrence-template")
        val cache = memoryCache { now }
        var gets = 0
        val transport = transport { req ->
            gets++
            val scope = req.url.parameters["occurrence_id"]
            val source = if (scope == null) template else base
            respond(WireJson.encodeToString(ListResponse.serializer(), source.copy(occurrenceId = scope,
                propertyEntries = source.propertyEntries.map { it.copy(occurrenceId = scope) })), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val mark = cache.receiptMark()
            val rows = (1..100).map { n -> base.copy(occurrenceId = "01999999-0000-7000-8000-${n.toString().padStart(12, '0')}",
                propertyEntries = base.propertyEntries.map { it.copy(occurrenceId = "01999999-0000-7000-8000-${n.toString().padStart(12, '0')}") }) }
            cache.putList(template)
            for (row in rows) { cache.putList(row); assertTrue(cache.view.value.lists.count { it.occurrenceId != null } <= 32) }
            assertTrue(cache.view.value.lists.any { it.occurrenceId == rows.last().occurrenceId })
            // The cap is household-wide, not 32 scopes for every individual list.
            val otherListId = "01999999-0000-7000-8000-000000001001"
            val otherScope = "01999999-0000-7000-8000-000000001002"
            cache.putList(base.copy(id = otherListId, occurrenceId = otherScope,
                propertyEntries = base.propertyEntries.map { it.copy(listId = otherListId, occurrenceId = otherScope) }))
            assertEquals(32, cache.view.value.lists.count { it.occurrenceId != null })
            repo(transport, cache).refetchByIds(setOf(base.id))
            assertTrue(gets <= 33, "One template plus at most 32 retained scopes")
            val live = rows.last()
            cache.installStaged(emptySnapshot(10).copy(items = listOf(itemRead(itemId)), lists = listOf(template),
                occurrences = listOf(occurrenceRead(live.occurrenceId!!, itemId))), receivedAfter = mark)
            assertEquals(setOf(null, live.occurrenceId), cache.view.value.lists.map { it.occurrenceId }.toSet())
            assertNotNull(cache.view.value.lists.single { it.occurrenceId != null }.propertyEntries.first().checkedAt)
            gets = 0
            repo(transport, cache).refetchByIds(setOf(base.id)); assertEquals(2, gets)
            cache.install(emptySnapshot(11).copy(items = listOf(itemRead(itemId)), lists = listOf(template)))
            assertEquals(listOf(null), cache.view.value.lists.map { it.occurrenceId })
            gets = 0
            repo(transport, cache).refetchByIds(setOf(base.id)); assertEquals(1, gets)
        } finally { cache.close(); transport.close() }
    }

    @Test fun cleanupDropsScopesOutsideOccurrenceCacheWindow() = runTest {
        val scoped = row("list-occurrence-check")
        val template = row("list-occurrence-template")
        val cache = memoryCache { now }
        try {
            val occurrence = occurrenceRead(scoped.occurrenceId!!, scoped.itemId!!).copy(completedAt = now)
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(scoped.itemId)), occurrences = listOf(occurrence), lists = listOf(template)))
            cache.putList(scoped)
            cache.cleanup(now + 181.days)
            assertTrue(cache.view.value.occurrences.isEmpty())
            assertEquals(listOf(null), cache.view.value.lists.map { it.occurrenceId })
        } finally { cache.close() }
    }

    @Test fun reopeningPreFixCacheBoundsScopesAndTheirEntryRowsBeforeFirstTickle() = runTest {
        val base = row("list-occurrence-check")
        val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(driver)
        val q = PorchlightDb(driver).cacheQueries
        // Same disposable schema as fix 1, whose on-disk history was unbounded.
        for (n in 1..100) {
            val scope = "01999999-0000-7000-8000-${n.toString().padStart(12, '0')}"
            val scoped = base.copy(occurrenceId = scope, propertyEntries = base.propertyEntries.map { it.copy(occurrenceId = scope) })
            q.putList(scoped.id, scope, scoped.itemId, null, WireJson.encodeToString(ListResponse.serializer(), scoped))
            scoped.propertyEntries.forEach { q.putEntry(it.id, scope, scoped.id, null, WireJson.encodeToString(EntryResponse.serializer(), it)) }
        }
        val cache = ReadCache(driver) { now }
        try {
            cache.initialize()
            assertEquals(32, cache.view.value.lists.size)
            assertEquals(32 * base.propertyEntries.size, q.allEntrys().executeAsList().size)
            assertTrue(cache.view.value.lists.any { it.occurrenceId!!.endsWith("000000000100") })
        } finally { cache.close() }
    }

    @Test fun cancellationNeverContinuesTheBatch() = runTest {
        val cache = memoryCache { now }
        var calls = 0
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): kotlinx.serialization.json.JsonObject {
                calls++; throw CancellationException("stop")
            }
        }
        try {
            assertFailsWith<CancellationException> { repo(transport, cache).refetchByIds(linkedSetOf("first", "second")) }
            assertEquals(1, calls)
        } finally { cache.close() }
    }
}
