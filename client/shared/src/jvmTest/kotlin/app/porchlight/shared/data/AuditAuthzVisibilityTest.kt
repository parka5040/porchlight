package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditAuthzVisibilityTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun row(name: String) = WireJson.decodeFromString<ListResponse>(checkNotNull(javaClass.getResource("/item-fixtures/$name.json")).readText())

    @Test fun sameVersionRedactionFencesDelayedSnapshotsAndDetailReads() = runTest {
        for (authoritative in listOf(false, true)) {
            val scoped = row("list-occurrence-check")
            val template = row("list-occurrence-template").copy(version = scoped.version)
            val redacted = row("list-orphan-redacted").copy(id = template.id, itemId = template.itemId,
                mode = ListMode.per_occurrence, version = template.version)
            val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
            PorchlightDb.Schema.create(driver)
            val q = PorchlightDb(driver).cacheQueries
            val cache = ReadCache(driver) { now }
            val detailStarted = CompletableDeferred<Unit>()
            val releaseDetail = CompletableDeferred<Unit>()
            var delayDetail = true
            var collection = redacted
            val transport = KtorApiTransport(HttpClient(MockEngine { req ->
                val body = if (req.url.encodedPath == "/v1/lists") {
                    "{\"items\":[${WireJson.encodeToString(ListResponse.serializer(), collection)}],\"next_cursor\":null}"
                } else {
                    if (delayDetail) { detailStarted.complete(Unit); releaseDetail.await() }
                    WireJson.encodeToString(ListResponse.serializer(), scoped)
                }
                respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            try {
                val snapshot = emptySnapshot(10).copy(items = listOf(itemRead(template.itemId!!)), lists = listOf(template),
                    occurrences = listOf(occurrenceRead(scoped.occurrenceId!!, template.itemId)))
                cache.install(snapshot)
                cache.putList(scoped)
                val mark = cache.snapshotMark()
                val snapshotStarted = CompletableDeferred<Unit>()
                val releaseSnapshot = CompletableDeferred<Unit>()
                val sync = launch {
                    snapshotStarted.complete(Unit)
                    releaseSnapshot.await() // The snapshot is read, projections are still in flight.
                    cache.installStaged(snapshot, receivedAfter = mark.receipts, requestCursor = mark.cursor, authoritative = authoritative)
                }
                snapshotStarted.await()
                val detail = async { repo.get(template.id, scoped.occurrenceId) }
                detailStarted.await()
                cache.putEntry(scoped.propertyEntries.first().copy(version = 20)) // newer entry receipt must be purged too
                repo.refresh(includeOrphaned = true)
                assertEquals(listOf(redacted), cache.view.value.lists)
                assertTrue(q.allEntrys().executeAsList().isEmpty())
                releaseSnapshot.complete(Unit); sync.join()
                assertEquals(listOf(redacted), cache.view.value.lists, "older same-version snapshot must not restore private content")
                assertTrue(q.allEntrys().executeAsList().isEmpty(), "snapshot must not restore entries or checks")
                releaseDetail.complete(Unit)
                assertEquals(redacted, detail.await(), "older detail response must not escape the visibility fence")
                assertEquals(listOf(redacted), cache.view.value.lists)
                assertTrue(q.allEntrys().executeAsList().isEmpty())
                // A second delayed snapshot still cannot bypass a fence cleared as an ordinary receipt.
                cache.installStaged(snapshot, receivedAfter = mark.receipts, requestCursor = mark.cursor)
                assertEquals(listOf(redacted), cache.view.value.lists)
                // Only a read started after the redaction may explicitly restore access (version may be equal).
                delayDetail = false
                assertEquals(scoped, repo.get(template.id, scoped.occurrenceId))
                assertTrue(cache.view.value.lists.any { it == scoped })
                repo.refresh(includeOrphaned = true)
                collection = template
                repo.refresh(includeOrphaned = true)
                assertEquals(template, cache.view.value.lists.single())
            } finally {
                releaseDetail.complete(Unit)
                transport.close(); cache.close()
            }
        }
    }
}
