package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.ScheduledReminderRead
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class AuditBlClient2bEvictionBudgetTest {
    @Test fun occurrenceEvictionReadsProjectionPayloadsOnlyForPublication() = runTest {
        val delegate = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(delegate)
        var payloadReads = 0
        val driver = object : SqlDriver by delegate {
            override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> {
                val projections = Regex("FROM Cache(Board|Agenda|Reminder) ORDER BY id", RegexOption.IGNORE_CASE).containsMatchIn(sql)
                return delegate.executeQuery(identifier, sql, { cursor -> mapper(object : SqlCursor by cursor {
                    override fun getString(index: Int): String? = cursor.getString(index).also {
                        if (projections && it?.startsWith("{") == true) payloadReads++
                    }
                }) }, parameters, binders)
            }
        }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = ReadCache(driver) { now }
        try {
            val ids = (0 until 1000).map { "occ-$it" }
            val items = ids.map { itemRead(it) }
            val occurrences = ids.map { occurrenceRead(it, it) }
            cache.install(emptySnapshot(1).copy(items = items, occurrences = occurrences,
                scheduledReminders = ids.map { ScheduledReminderRead("r-$it", "h", "self", it, it, "offset", now + 1.days, now) }))
            cache.replaceBoard(items.mapIndexed { index, item -> item.copy(occurrence = occurrences[index]) })
            cache.replaceAgenda(ids.map { agenda(it, item = it) })
            payloadReads = 0
            cache.evictOccurrence(ids.first())
            assertTrue(payloadReads <= 3000, "projection payload reads: $payloadReads; only publication may decode unrelated rows")
            assertEquals(999, cache.view.value.board.size)
            assertEquals(999, cache.view.value.agenda.size)
            assertEquals(999, cache.view.value.reminders.size)
        } finally { cache.close() }
    }
}
