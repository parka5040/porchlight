package app.porchlight.shared.data

import android.content.Context
import android.database.sqlite.SQLiteDatabaseCorruptException
import androidx.sqlite.db.SupportSQLiteDatabase
import app.cash.sqldelight.driver.android.AndroidSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import java.io.File
import kotlin.time.Instant

/** Only the disposable read DB is eligible for recreation; the private alarm/trust stores are unrelated. */
fun androidReadCache(context: Context, attachmentStore: AttachmentStore = MemoryAttachmentStore(), now: () -> Instant): ReadCache {
    val app = context.applicationContext
    val schema = PorchlightDb.Schema
    val callback = object : AndroidSqliteDriver.Callback(schema) {
        override fun onUpgrade(db: SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = recreate(db)
        override fun onDowngrade(db: SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = recreate(db)
        // Keep recovery bounded by ReadCache rather than the open helper's automatic deletion/reopen path.
        override fun onCorruption(db: SupportSQLiteDatabase) { throw SQLiteDatabaseCorruptException("Disposable cache is corrupt") }
        private fun recreate(db: SupportSQLiteDatabase) {
            val tables = db.query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> 'android_metadata'").use { cursor ->
                buildList { while (cursor.moveToNext()) add(cursor.getString(0)) }
            }
            tables.forEach { db.execSQL("DROP TABLE IF EXISTS \"$it\"") }
            onCreate(db)
        }
    }
    return ReadCache({ AndroidSqliteDriver(schema, app, "read-cache.db", callback = callback) }, now, object : CacheRecovery {
        override fun isCorruption(error: Exception): Boolean = generateSequence<Throwable>(error) { it.cause }
            .any { it is SQLiteDatabaseCorruptException }
        override fun discard() {
            val file = app.getDatabasePath("read-cache.db")
            for (suffix in listOf("", "-wal", "-shm", "-journal")) {
                val part = File(file.absolutePath + suffix)
                check(!part.exists() || part.delete()) { "Unable to recreate read cache" }
            }
        }
    }, attachmentStore)
}
