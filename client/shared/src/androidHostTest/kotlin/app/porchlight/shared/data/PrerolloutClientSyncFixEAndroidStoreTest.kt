package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutClientSyncFixEAndroidStoreTest {
    @Test fun failedEnumerationRetainsViewerObligationAcrossRestartAndClearAlsoFails() = runTest {
        val dir = Files.createTempDirectory(File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }.toPath(), "android-purge-").toFile()
        val root = File(dir, "attachments").apply { writeText("not a directory") }
        val file = File(dir, "cache.db")
        JdbcSqliteDriver("jdbc:sqlite:$file").let { PorchlightDb.Schema.create(it); it.close() }
        val store = FileAttachmentStore(root)
        var at = Instant.parse("2026-01-01T00:00:00Z")
        fun open() = ReadCache({ JdbcSqliteDriver("jdbc:sqlite:$file") }, { at }, null, store)
        var cache = open()
        try {
            val driver = JdbcSqliteDriver("jdbc:sqlite:$file")
            val q = PorchlightDb(driver).cacheQueries
            q.putFileNamespace(1); q.putSync(1, 0); q.putAttachmentCleanup("view", "attachment-a")
            assertFailsWith<IllegalStateException> { cache.acknowledgeCursor(2) }
            assertEquals(1, q.allAttachmentCleanup().executeAsList().size)
            assertFailsWith<IllegalStateException> { store.clear() }
            cache.close(); cache = open()
            assertFailsWith<IllegalStateException> { cache.initialize() }
            assertEquals(1, q.allAttachmentCleanup().executeAsList().size)
            check(root.delete()); check(root.mkdir()); File(root, "attachment-a-view.pdf").writeText("private")
            at = Instant.parse("2026-01-02T00:00:00Z")
            cache.initialize(); cache.acknowledgeCursor(2)
            assertTrue(q.allAttachmentCleanup().executeAsList().isEmpty())
            assertTrue(root.listFiles()!!.isEmpty())
            driver.close()
        } finally { cache.close(); dir.deleteRecursively() }
    }
}
