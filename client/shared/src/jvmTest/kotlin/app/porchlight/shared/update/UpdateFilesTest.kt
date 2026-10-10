package app.porchlight.shared.update

import app.porchlight.shared.api.models.ReleaseFile
import app.porchlight.shared.backups.DownloadHead
import app.porchlight.shared.transport.ConnectFailure
import kotlinx.coroutines.runBlocking
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest
import kotlin.test.*

internal fun updateTestRoot(): Path = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "updates-")
internal fun sha(bytes: ByteArray) = digestHex(MessageDigest.getInstance("SHA-256").digest(bytes))
class UpdateFilesTest {
    private val bytes = "signed fixture bytes".toByteArray()
    private val file get() = ReleaseFile("lib/operations.py", bytes.size.toLong(), sha(bytes))
    private fun source(reply: suspend (Long, suspend (DownloadHead) -> Unit, suspend (ByteArray) -> Unit) -> Unit) = object : TestUpdateSource() {
        override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) = reply(offset, begin, chunk)
    }
    @Test fun resumeHashesCheckpointAndStreamsRemainderWithOwnerOnlyFiles() = runBlocking {
        val root = updateTestRoot()
        var attempts = 0
        val source = source { offset, begin, chunk ->
            attempts++
            if (attempts == 1) {
                assertEquals(0, offset); begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null))
                chunk(bytes.copyOf(5)); throw ConnectFailure()
            } else {
                assertEquals(5, offset)
                begin(DownloadHead(206, "\"${file.sha256}\"", file.propertySize - offset, "bytes $offset-${file.propertySize - 1}/${file.propertySize}"))
                chunk(bytes.copyOfRange(5, bytes.size))
            }
        }
        try {
            assertFailsWith<ConnectFailure> { downloadReleaseFiles(source, listOf(file), root, {}) }
            val result = downloadReleaseFiles(source, listOf(file), root, {})
            assertContentEquals(bytes, Files.readAllBytes(result.resolve(file.path)))
            assertEquals(PosixFilePermissions.fromString("rwx------"), Files.getPosixFilePermissions(result))
            assertEquals(PosixFilePermissions.fromString("rw-------"), Files.getPosixFilePermissions(result.resolve(file.path)))
            assertFalse(Files.exists(root.resolve("parts")))
        } finally { removeUpdateTree(root) }
    }
    @Test fun resumedRequestMayRestartOn200AndWholeWrongHashIsDeleted() = runBlocking {
        val root = updateTestRoot()
        try {
            val partial = guardedUpdateDirectory(root.resolve("parts")).resolve(file.sha256)
            Files.write(partial, bytes.copyOf(3))
            var offsetSeen = -1L
            downloadReleaseFiles(source { offset, begin, chunk -> offsetSeen = offset; begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null)); chunk(bytes) }, listOf(file), root, {})
            assertEquals(3, offsetSeen)
            Files.delete(root.resolve("source/${file.path}"))
            val wrong = bytes.copyOf().also { it[0] = 0 }
            val failure = assertFailsWith<UpdateFailure> {
                downloadReleaseFiles(source { _, begin, chunk -> begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null)); chunk(wrong) }, listOf(file), root, {})
            }
            assertEquals(DOWNLOAD_MISMATCH, failure.reason)
            assertFalse(Files.exists(partial)); assertFalse(Files.exists(root.resolve("source/${file.path}")))
        } finally { removeUpdateTree(root) }
    }
    @Test fun wrongRangeEtagOversizeAndChunkBeforeHeadNeverPublish() = runBlocking {
        for (case in 0..4) {
            val root = updateTestRoot()
            try {
                val partial = guardedUpdateDirectory(root.resolve("parts")).resolve(file.sha256)
                Files.write(partial, bytes.copyOf(3))
                assertFailsWith<UpdateFailure> {
                    downloadReleaseFiles(source { _, begin, chunk ->
                        when (case) {
                            0 -> begin(DownloadHead(206, "\"wrong\"", file.propertySize - 3, "bytes 3-${file.propertySize - 1}/${file.propertySize}"))
                            1 -> begin(DownloadHead(206, "\"${file.sha256}\"", file.propertySize - 3, "bytes 4-${file.propertySize - 1}/${file.propertySize}"))
                            2 -> { begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null)); chunk(ByteArray(bytes.size + 1)) }
                            3 -> chunk(bytes)
                            else -> begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize + 1, null))
                        }
                    }, listOf(file), root, {})
                }
                assertFalse(Files.exists(root.resolve("source/${file.path}")))
            } finally { removeUpdateTree(root) }
        }
    }
    @Test fun pathAndSymlinkGuardsDoNotWriteOutsideStaging() {
        val root = updateTestRoot()
        try {
            assertFailsWith<IllegalArgumentException> { updatePath(root, "../escape") }
            val external = Files.createDirectory(root.resolve("outside"))
            Files.createSymbolicLink(root.resolve("link"), external)
            assertFailsWith<IllegalArgumentException> { updatePath(root, "link/file") }
            assertFailsWith<IllegalArgumentException> { guardedUpdateDirectory(root.resolve("link/child")) }
            assertFalse(Files.exists(external.resolve("file")))
            assertFailsWith<IllegalArgumentException> { validateReleaseFiles(listOf(file, file)) }
        } finally { removeUpdateTree(root) }
    }
    @Test fun linkedPlatformPrefixIsCanonicalizedButLinksBelowTheRootStayRefused() {
        val root = updateTestRoot()
        try {
            // Like Android's /data/user/0 -> /data/data: a link above the update root is platform-owned.
            val real = Files.createDirectory(root.resolve("data"))
            Files.createSymbolicLink(root.resolve("user0"), real)
            assertFailsWith<IllegalArgumentException> { guardedUpdateDirectory(root.resolve("user0/app/updates")) }
            val updates = realUpdateRoot(root.resolve("user0/app/updates"))
            assertEquals(real.toRealPath().resolve("app/updates"), updates)
            val staged = guardedUpdateDirectory(guardedUpdateDirectory(updates).resolve("9.9.9"))
            assertTrue(Files.isDirectory(real.resolve("app/updates/9.9.9"), LinkOption.NOFOLLOW_LINKS))
            Files.createSymbolicLink(updates.resolve("lib"), Files.createDirectory(root.resolve("elsewhere")))
            assertFailsWith<IllegalArgumentException> { guardedUpdateDirectory(updates.resolve("lib/child")) }
            assertFalse(Files.exists(root.resolve("elsewhere/child")))
            assertEquals(staged, updates.resolve("9.9.9"))
        } finally { removeUpdateTree(root) }
    }
}
