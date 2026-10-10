package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.cash.sqldelight.db.QueryResult
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant

class DesktopCacheTest {
    @Test fun reopenRestoresCommittedCacheAndSchemaMismatchRecreatesOnlyReadCache() = runTest {
        val root = File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }
        val dir = Files.createTempDirectory(root.toPath(), "cache-").toFile()
        val file = File(dir, "read-cache.db")
        val sentinel = File(dir, "device.preferences_pb").apply { writeText("keep") }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        try {
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.install(emptySnapshot(31).copy(items = listOf(itemRead("persisted"))))
                cache.close()
            }
            var opened = false
            // Production passes the opener: file bootstrap waits for initialize() on Dispatchers.Default (m-14).
            ReadCache({ opened = true; openDesktopCache(file) }, { now }).let { cache ->
                assertFalse(opened)
                assertNull(cache.sync.value) // Disk reads are explicit and run off-main.
                cache.initialize()
                assertTrue(opened)
                assertEquals(31, cache.sync.value?.cursor)
                assertEquals("persisted", cache.view.value.items.single().id)
                cache.close()
            }
            JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath).let { driver ->
                driver.execute(null, "PRAGMA user_version = 999", 0)
                driver.close()
            }
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.initialize(); assertNull(cache.sync.value); assertTrue(cache.view.value.items.isEmpty()); cache.close()
            }
            assertEquals("keep", sentinel.readText())
            val driver = openDesktopCache(file)
            val version = driver.executeQuery(null, "PRAGMA user_version", { cursor ->
                cursor.next(); QueryResult.Value(cursor.getLong(0)!!)
            }, 0).value
            assertEquals(PorchlightDb.Schema.version, version)
            driver.close()
        } finally { dir.deleteRecursively() }
    }
}
