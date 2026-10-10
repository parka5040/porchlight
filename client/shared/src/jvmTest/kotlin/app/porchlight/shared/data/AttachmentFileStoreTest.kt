package app.porchlight.shared.data

import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermission
import kotlin.test.*
import kotlin.time.Instant

class AttachmentFileStoreTest {
    @Test fun atomicPrivateFilesRejectPathsExpireAndClear() = runTest {
        val base = Path.of(System.getProperty("porchlight.test.dir")); Files.createDirectories(base)
        val directory = Files.createTempDirectory(base, "c5-attachment-").toFile()
        try {
            val store = FileAttachmentStore(directory)
            assertFailsWith<IllegalArgumentException> { store.write("../outside", byteArrayOf(1)) }
            store.write("0-attachment-1-file", "hello".encodeToByteArray())
            assertEquals("hello", store.read("0-attachment-1-file")?.decodeToString())
            assertEquals(setOf(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE, PosixFilePermission.OWNER_EXECUTE), Files.getPosixFilePermissions(directory.toPath()))
            val file = directory.resolve("0-attachment-1-file")
            assertEquals(setOf(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE), Files.getPosixFilePermissions(file.toPath()))
            assertTrue(directory.listFiles()!!.none { it.name.endsWith("-part") })
            file.setLastModified(1)
            store.cleanup(Instant.parse("2026-10-06T12:00:00Z"))
            assertNull(store.read("0-attachment-1-file"))
            store.write("0-attachment-2-file", byteArrayOf(2)); store.clear()
            assertTrue(directory.listFiles()!!.isEmpty())
        } finally { directory.deleteRecursively() }
    }
}
