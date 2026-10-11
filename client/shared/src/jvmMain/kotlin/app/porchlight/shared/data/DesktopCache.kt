package app.porchlight.shared.data

import app.cash.sqldelight.db.QueryResult
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import java.io.File
import kotlin.time.Instant

fun desktopReadCache(file: File, attachmentStore: AttachmentStore = MemoryAttachmentStore(), now: () -> Instant): ReadCache =
    ReadCache({ openDesktopCache(file) }, now, object : CacheRecovery {
        override fun isCorruption(error: Exception): Boolean = generateSequence<Throwable>(error) { it.cause }.any {
            // Primary SQLite codes: CORRUPT / NOTADB only, including their extended codes. Never IOERR/FULL/BUSY.
            it is java.sql.SQLException && (it.errorCode and 0xff) in setOf(11, 26)
        }
        override fun discard() = discardDesktopCache(file)
    }, attachmentStore)

private fun discardDesktopCache(file: File) {
    for (suffix in listOf("", "-wal", "-shm", "-journal")) {
        val part = File(file.absolutePath + suffix)
        check(!part.exists() || part.delete()) { "Unable to recreate read cache" }
    }
}

/** Rebuildable projections, never authoritative state. Unknown/newer schemas are recreated, not guessed. */
fun openDesktopCache(file: File): JdbcSqliteDriver {
    file.parentFile.mkdirs()
    fun connect() = JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath)
    var driver = connect()
    try {
        val version = driver.executeQuery(null, "PRAGMA user_version", { cursor ->
            cursor.next(); QueryResult.Value(cursor.getLong(0) ?: 0L)
        }, 0).value
        if (version != PorchlightDb.Schema.version) {
            driver.close()
            discardDesktopCache(file)
            driver = connect()
            PorchlightDb.Schema.create(driver)
            driver.execute(null, "PRAGMA user_version = ${PorchlightDb.Schema.version}", 0)
        }
        return driver
    } catch (e: Exception) {
        driver.close()
        throw e
    }
}
