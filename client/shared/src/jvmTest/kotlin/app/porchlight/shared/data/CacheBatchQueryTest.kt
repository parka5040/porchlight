package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class CacheBatchQueryTest {
    @Test fun tenThousandOccurrencesUseConstantFullScansPerBatch() = runTest {
        val delegate = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(delegate)
        var scans = 0
        val driver = object : SqlDriver by delegate {
            override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> {
                if (Regex("FROM Cache(Item|Occurrence) ORDER BY id", RegexOption.IGNORE_CASE).containsMatchIn(sql)) scans++
                return delegate.executeQuery(identifier, sql, mapper, parameters, binders)
            }
        }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = ReadCache(driver) { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = (0 until 10_000).map { occurrenceRead("occ-$it", "one").copy(startsAt = now) })
            cache.install(snapshot)
            assertEquals(10_000, cache.view.value.occurrences.size)
            scans = 0
            cache.putOccurrences("one", (0 until 730).map { occurrenceResponse("occ-$it", "one").copy(version = 2, startsAt = now) }, now, now)
            assertTrue(scans <= 3, "batch full-table scans: $scans (expected cleanup plus publication scans)")
            assertEquals(10_000, cache.view.value.occurrences.size)
            scans = 0
            cache.install(snapshot.copy(cursor = 2))
            assertTrue(scans <= 3, "snapshot full-table scans: $scans")
        } finally { cache.close() }
    }
}
