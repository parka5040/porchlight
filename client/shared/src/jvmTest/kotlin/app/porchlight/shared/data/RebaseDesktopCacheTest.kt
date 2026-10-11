package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.AvailabilityResponse
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant

class RebaseDesktopCacheTest {
    @Test fun mainVersionFourCacheRecreatesWithCurrentSchema() = runTest {
        assertEquals(10L, PorchlightDb.Schema.version)
        val root = File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }
        val dir = Files.createTempDirectory(root.toPath(), "rebase-cache-").toFile()
        val file = File(dir, "read-cache.db")
        val sentinel = File(dir, "device.preferences_pb").apply { writeText("keep") }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        try {
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.install(emptySnapshot(3).copy(lists = listOf(listFixture())))
                cache.close()
            }
            // Main's v4 has planning/collaboration but no C4 notification tables.
            JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath).let { driver ->
                driver.execute(null, "DROP TABLE CacheNotification", 0)
                driver.execute(null, "DROP TABLE CacheNotificationPrefs", 0)
                driver.execute(null, "PRAGMA user_version = 4", 0)
                driver.close()
            }
            val availability = AvailabilityResponse(collabId("self"), 1, emptyList(), emptyList())
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.initialize()
                assertEquals(CacheView(), cache.view.value)
                cache.install(emptySnapshot(4).copy(availability = listOf(availability), lists = listOf(listFixture())))
                cache.close()
            }
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.initialize()
                assertEquals(listOf(availability), cache.view.value.availability)
                assertEquals(listOf(listFixture()), cache.view.value.lists)
                cache.close()
            }
            assertEquals("keep", sentinel.readText())
        } finally { dir.deleteRecursively() }
    }
}
