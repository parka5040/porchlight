package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientBAttachmentTest {
    @Test fun failedPurgeCannotDeleteReauthorizedBytesOrPreparedViewer() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "final-view-").toFile()
        val disk = FileAttachmentStore(root)
        var unavailable = true
        val store = object : AttachmentStore by disk {
            override suspend fun remove(key: String) { if (unavailable) error("Storage temporarily unavailable"); disk.remove(key) }
            override suspend fun removeView(id: String) { if (unavailable) error("Storage temporarily unavailable"); disk.removeView(id) }
        }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache(store) { now }
        try {
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            val key = attachmentKey(0, row, false)
            disk.write(key, byteArrayOf(1, 2, 3))
            assertFailsWith<IllegalStateException> { cache.evictAttachment(row.id) }
            unavailable = false
            cache.inSession { cache.putAttachment(it, row) }
            val data = HouseholdData(cache, GeneratedCall({ error("Cache hit must not use network") }, { null }), { now })
            var launched = false
            data.attachments.openPrepared(row) { bytes ->
                val viewer = root.resolve("${row.id}-view.txt").apply { writeBytes(bytes) }
                object : PreparedAttachmentViewer {
                    override suspend fun launch() {
                        assertTrue(viewer.exists(), "Queued obsolete ID purge deleted the authorized viewer before launch")
                        assertContentEquals(bytes, disk.read(key), "Queued byte deletion must respect restored current references")
                        launched = true
                    }
                    override suspend fun discard() { viewer.delete() }
                }
            }
            assertTrue(launched)
        } finally { cache.close(); root.deleteRecursively() }
    }
}
