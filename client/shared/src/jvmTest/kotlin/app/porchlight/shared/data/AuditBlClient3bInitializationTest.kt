package app.porchlight.shared.data

import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient3bInitializationTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun externalCursorResetInvalidatesTheStorageFence() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-initialize-")
        val file = root.resolve("cache.db").toFile()
        val cache = desktopReadCache(file) { now }
        try {
            cache.install(emptySnapshot(1))
            cache.initialize()
            val revision = cache.storageRevision.value
            val external = openDesktopCache(file)
            try { PorchlightDb(external).cacheQueries.clearSync() } finally { external.close() }
            cache.initialize()
            assertNull(cache.sync.value, "External cursor loss must invalidate the in-memory snapshot")
            assertTrue(cache.storageRevision.value > revision, "A healthy stream must reconnect after cache loss")
        } finally { cache.close(); root.toFile().deleteRecursively() }
    }

}
