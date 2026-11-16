package app.porchlight.shared.update

import app.porchlight.shared.api.models.ReleaseFile
import app.porchlight.shared.backups.DownloadHead
import kotlinx.coroutines.runBlocking
import java.nio.file.*
import kotlin.test.*

class PrerolloutDesktopFix1SpaceTest {
    @Test fun ninetyPercentCheckpointNeedsOnlyRemainderPlusMargin() = runBlocking {
        val root = updateTestRoot()
        val bytes = ByteArray(100) { it.toByte() }
        val file = ReleaseFile("archive", 100, sha(bytes))
        val source = object : TestUpdateSource() {
            override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                assertEquals(90, offset)
                begin(DownloadHead(206, "\"${file.sha256}\"", 10, "bytes 90-99/100"))
                chunk(bytes.copyOfRange(90, 100))
            }
        }
        try {
            Files.write(guardedUpdateDirectory(root.resolve("parts")).resolve(file.sha256), bytes.copyOf(90))
            val result = downloadReleaseFiles(source, listOf(file), root, {}, availableSpace = { 64L * 1024 * 1024 + 10 })
            assertContentEquals(bytes, Files.readAllBytes(result.resolve(file.path)))
        } finally { removeUpdateTree(root) }
    }
    @Test fun validatedCompleteTargetNeedsNoPayloadSpace() = runBlocking {
        val root = updateTestRoot()
        val bytes = "complete".toByteArray()
        val file = ReleaseFile("archive", bytes.size.toLong(), sha(bytes))
        try {
            Files.write(guardedUpdateDirectory(root.resolve("source")).resolve(file.path), bytes)
            downloadReleaseFiles(object : TestUpdateSource() {}, listOf(file), root, {}, availableSpace = { 64L * 1024 * 1024 })
            Unit
        } finally { removeUpdateTree(root) }
    }
    @Test fun shortageOfEvenOneRemainingByteFailsBeforeNetwork() = runBlocking {
        val root = updateTestRoot()
        val bytes = ByteArray(100)
        val file = ReleaseFile("archive", 100, sha(bytes))
        try {
            val part = Files.write(guardedUpdateDirectory(root.resolve("parts")).resolve(file.sha256), bytes.copyOf(90))
            assertFailsWith<UpdateFailure> {
                downloadReleaseFiles(object : TestUpdateSource() {}, listOf(file), root, {}, availableSpace = { 64L * 1024 * 1024 + 9 })
            }
            assertEquals(90, Files.size(part))
        } finally { removeUpdateTree(root) }
    }
}
