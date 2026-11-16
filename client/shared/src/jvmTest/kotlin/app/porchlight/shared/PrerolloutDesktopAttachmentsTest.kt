package app.porchlight.shared

import app.porchlight.shared.data.*
import kotlinx.coroutines.runBlocking
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutDesktopAttachmentsTest {
    @Test fun budgetEvictsLeastRecentlyUsedAndStreamingNeverNeedsByteAdapter() = runBlocking {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "cache-budget-")
        try {
            val store = FileAttachmentStore(root.toFile(), byteBudget = 8)
            store.write("a", ByteArray(4)); store.write("b", ByteArray(4)); store.read("a")
            val staged = store.stage("c") { consume -> repeat(4) { consume(byteArrayOf(1)) } }
            assertEquals(2, root.toFile().listFiles()!!.count { !it.name.endsWith("-part") })
            staged.publish()
            assertNull(store.read("b")); assertNotNull(store.read("a"))
            var received = 0
            store.content("c")!!.chunks { received += it.size }
            assertEquals(4, received)
            assertEquals(8L, root.toFile().listFiles()!!.sumOf { it.length() })
            assertTrue(Files.getPosixFilePermissions(Path.of(staged.privatePath!!)).all { it.name.startsWith("OWNER_") })
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun byteCacheRefusesSymlinkRootBeforeWriting() = runBlocking {
        val parent = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "private-cache-")
        try {
            val outside = Files.createDirectory(parent.resolve("outside"))
            val link = Files.createSymbolicLink(parent.resolve("cache"), outside)
            assertFails { FileAttachmentStore(link.toFile()).write("original", byteArrayOf(1)) }
            assertEquals(0L, Files.list(outside).use { it.count() })
        } finally { parent.toFile().deleteRecursively() }
    }
    @Test fun predictableStagingLinkCannotReceiveBytes() = runBlocking {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "cache-stage-")
        try {
            val target = Files.write(root.resolve("outside"), byteArrayOf(7))
            Files.createSymbolicLink(root.resolve("original-part"), target)
            FileAttachmentStore(root.toFile()).write("original", byteArrayOf(1))
            assertContentEquals(byteArrayOf(7), Files.readAllBytes(target))
        } finally { root.toFile().deleteRecursively() }
    }
}
