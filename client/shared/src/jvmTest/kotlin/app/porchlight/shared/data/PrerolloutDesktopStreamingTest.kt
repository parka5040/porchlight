package app.porchlight.shared.data

import app.porchlight.shared.backups.DownloadHead
import app.porchlight.shared.transport.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutDesktopStreamingTest {
    @Test fun originalViewerStreamsToPrivateIdentityFileAndCleanupIsAmortized() = runBlocking {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "stream-original-")
        val disk = FileAttachmentStore(root.toFile())
        var cleanups = 0; var downloads = 0
        val store = object : AttachmentStore by disk {
            override suspend fun cleanup(before: Instant) { cleanups++; disk.cleanup(before) }
        }
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache(store) { now }
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = JsonObject(emptyMap())
            override suspend fun download(path: String, session: String?): ByteArray = error("Original must not use the byte-buffer adapter")
            override suspend fun download(path: String, session: String?, range: String?, ifRange: String?, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                downloads++; begin(DownloadHead(200, null, 262_144, null))
                repeat(4) { chunk(ByteArray(65_536) { 1 }) }
            }
        }
        try {
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            repeat(20) {
                data.attachments.openPreparedContent(row) { content ->
                    assertNotNull(content.privatePath)
                    assertTrue(Path.of(content.privatePath!!).fileName.toString().startsWith("${cache.attachmentNamespace}-"))
                    var count = 0
                    content.chunks { count += it.size }
                    assertEquals(262_144, count)
                    object : PreparedAttachmentViewer {
                        override suspend fun launch() = Unit
                        override suspend fun discard() = Unit
                    }
                }
            }
            assertEquals(1, downloads)
            assertEquals(1, cleanups, "Cache hits must not scan the directory before every open")
            assertEquals(262_144L, Files.list(root).use { files -> files.mapToLong { Files.size(it) }.sum() })
        } finally { cache.close(); root.toFile().deleteRecursively() }
    }
}
