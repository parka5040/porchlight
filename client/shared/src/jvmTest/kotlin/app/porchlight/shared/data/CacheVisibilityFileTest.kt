package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class CacheVisibilityFileTest {
    @Test fun itemCommentAndDownload404RemoveOriginalThumbnailAndViewerFromDisk() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val base = Path.of(System.getProperty("porchlight.test.dir")); Files.createDirectories(base)
        for (endpoint in listOf("item", "comment", "download")) {
            val root = Files.createTempDirectory(base, "ccache-visibility-").toFile()
            val cache = memoryCache { now }
            val api = KtorApiTransport(HttpClient(MockEngine {
                respond("""{"type":"urn:porchlight:problem:not-found","status":404,"title":"Unavailable"}""", HttpStatusCode.NotFound, headersOf(HttpHeaders.ContentType, "application/problem+json"))
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
            try {
                val row = attachmentFixture().copy(hasThumbnail = true)
                val store = FileAttachmentStore(root)
                val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }, attachmentStore = store)
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), comments = listOf(commentFixture()), attachments = listOf(row)))
                // Download requests the absent thumbnail; the already viewed original and viewer must still purge.
                store.write("0-${row.id}-${row.version}-file", byteArrayOf(1))
                if (endpoint != "download") store.write("0-${row.id}-${row.version}-thumb", byteArrayOf(2))
                root.resolve("${row.id}-view.txt").writeText("private viewer copy")
                assertFailsWith<ApiProblem> { when (endpoint) {
                    "item" -> data.items.get(row.itemId)
                    "comment" -> data.comments.refresh(row.itemId)
                    else -> data.attachments.download(row, thumbnail = true)
                } }
                assertTrue(root.listFiles()!!.isEmpty(), "$endpoint left attachment files")
                assertTrue(cache.view.value.attachments.isEmpty()); assertTrue(cache.view.value.comments.isEmpty())
            } finally { api.close(); cache.close(); root.deleteRecursively() }
        }
    }
}
