package app.porchlight.shared.data

import app.cash.sqldelight.db.QueryResult
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import java.io.File

/** Rebuildable projections, never authoritative state. Unknown/newer schemas are recreated, not guessed. */
fun openDesktopCache(file: File): JdbcSqliteDriver {
    file.parentFile.mkdirs()
    fun connect() = JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath)
    var driver = connect()
    val version = driver.executeQuery(null, "PRAGMA user_version", { cursor ->
        cursor.next(); QueryResult.Value(cursor.getLong(0) ?: 0L)
    }, 0).value
    if (version != PorchlightDb.Schema.version) {
        driver.close()
        check(!file.exists() || file.delete()) { "Unable to recreate read cache" }
        for (suffix in listOf("-wal", "-shm", "-journal")) {
            val sidecar = File(file.absolutePath + suffix)
            check(!sidecar.exists() || sidecar.delete()) { "Unable to recreate read cache" }
        }
        driver = connect()
        PorchlightDb.Schema.create(driver)
        driver.execute(null, "PRAGMA user_version = ${PorchlightDb.Schema.version}", 0)
    }
    return driver
}
