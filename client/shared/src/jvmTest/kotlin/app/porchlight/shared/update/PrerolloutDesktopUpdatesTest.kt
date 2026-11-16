package app.porchlight.shared.update

import app.porchlight.shared.api.models.ReleaseFile
import app.porchlight.shared.backups.DownloadHead
import kotlinx.coroutines.runBlocking
import kotlin.test.*

class PrerolloutDesktopUpdatesTest {
    @Test fun noFreeSpaceFailsBeforeAnyNetworkOrStagingFiles() = runBlocking {
        val root = updateTestRoot()
        try {
            val failure = assertFailsWith<UpdateFailure> {
                downloadReleaseFiles(object : TestUpdateSource() {}, listOf(ReleaseFile("archive", 1, "a".repeat(64))), root.resolve("new"), {}, availableSpace = { 0 })
            }
            assertTrue(failure.reason.contains("free space"))
            assertFalse(java.nio.file.Files.exists(root.resolve("new")))
        } finally { removeUpdateTree(root) }
    }
    @Test fun fastChunkBurstDoesNotPublishProgressAtNetworkChunkRate() = runBlocking {
        val root = updateTestRoot()
        val bytes = ByteArray(1_000) { 1 }
        val file = ReleaseFile("archive", bytes.size.toLong(), sha(bytes))
        val source = object : TestUpdateSource() {
            override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null))
                repeat(1_000) { chunk(byteArrayOf(1)) }
            }
        }
        try {
            var published = 0
            downloadReleaseFiles(source, listOf(file), root, { if (it is UpdateState.Downloading) published++ }, nanoTime = { 0 })
            assertTrue(published <= 10, "A sub-second download must publish at most ten progress updates, not $published")
        } finally { removeUpdateTree(root) }
    }
}
