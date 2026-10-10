package app.porchlight.shared.files

import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.attribute.PosixFilePermission
import java.nio.file.attribute.PosixFilePermissions
import kotlin.test.*

/** fbcc c5-rr1 OOS: decrypted attachment plaintext for the desktop viewer is never readable by other users. */
class DesktopPrivateFilesTest {
    private val ownerFile = PosixFilePermissions.fromString("rw-------")
    private val ownerDir = PosixFilePermissions.fromString("rwx------")
    private fun base() = Files.createTempDirectory(
        File(System.getProperty("porchlight.test.dir"), "desktop-private").apply { mkdirs() }.toPath(), "viewer-"
    ).toFile()
    private fun mode(f: File) = Files.getPosixFilePermissions(f.toPath(), LinkOption.NOFOLLOW_LINKS)
    private fun shared(perms: Set<PosixFilePermission>) = perms.any { it.name.startsWith("GROUP") || it.name.startsWith("OTHERS") }

    @Test fun plaintextIsOwnerOnlyFromItsFirstByteInAPrivateDirectory() {
        val root = File(base(), "porchlight/attachments").apply { mkdirs() }
        // A cache dir left world-readable (older build, ambient 022 umask) is narrowed before any plaintext lands.
        Files.setPosixFilePermissions(root.toPath(), PosixFilePermissions.fromString("rwxr-xr-x"))
        val bytes = "decrypted attachment".encodeToByteArray()
        val file = writePrivateDesktopFile(root, "abc-view.txt") { out ->
            out.write(bytes, 0, 4)
            assertEquals(ownerDir, mode(root), "directory must be private before plaintext is written")
            root.listFiles()!!.filter { it.isFile }.forEach { assertFalse(shared(mode(it)), "${it.name} readable by others while being written: ${mode(it)}") }
            out.write(bytes, 4, bytes.size - 4)
        }
        assertEquals(File(root, "abc-view.txt"), file)
        assertContentEquals(bytes, file.readBytes())
        assertEquals(ownerFile, mode(file))
        assertEquals(ownerDir, mode(root))
        assertEquals(listOf("abc-view.txt"), root.list()!!.toList(), "no staging leftovers")
    }

    @Test fun freshCacheDirectoriesAreCreatedOwnerOnly() {
        val parent = base()
        Files.setPosixFilePermissions(parent.toPath(), PosixFilePermissions.fromString("rwxr-xr-x"))
        val root = File(parent, "porchlight/attachments")
        writePrivateDesktopFile(root, "abc-view.pdf") { it.write(1) }
        assertEquals(ownerDir, mode(root))
        assertEquals(ownerDir, mode(root.parentFile), "created intermediate directory")
    }

    @Test fun aPlantedSymlinkIsReplacedNeverWrittenThrough() {
        val dir = base()
        val outside = File(dir, "outside.txt").apply { writeText("keep") }
        val root = File(dir, "attachments").apply { mkdirs() }
        Files.createSymbolicLink(File(root, "abc-view.txt").toPath(), outside.toPath())
        val file = writePrivateDesktopFile(root, "abc-view.txt") { it.write("secret".encodeToByteArray()) }
        assertEquals("keep", outside.readText())
        assertFalse(Files.isSymbolicLink(file.toPath()))
        assertEquals("secret", file.readText())
        assertEquals(ownerFile, mode(file))
    }

    @Test fun aSymlinkedCacheDirectoryIsRefused() {
        val dir = base()
        val elsewhere = File(dir, "elsewhere").apply { mkdirs() }
        val root = File(dir, "attachments")
        Files.createSymbolicLink(root.toPath(), elsewhere.toPath())
        val failure = assertFailsWith<PrivateDirectoryUnavailable> { writePrivateDesktopFile(root, "abc-view.txt") { it.write("secret".encodeToByteArray()) } }
        assertEquals(root.path, failure.path)
        assertTrue(elsewhere.list()!!.isEmpty(), "nothing written through the link")
    }

    // fbcc fix 2: persistent directory problems are typed, so the viewer can say what is wrong instead of "retry".
    @Test fun aNonDirectoryOrUnwritableCacheLocationIsReportedAsADirectoryProblem() {
        val dir = base()
        val notADirectory = File(dir, "attachments").apply { writeText("x") }
        assertEquals(notADirectory.path, assertFailsWith<PrivateDirectoryUnavailable> { writePrivateDesktopFile(notADirectory, "a-view.txt") { it.write(1) } }.path)
        val parent = File(dir, "readonly").apply { mkdirs() }
        Files.setPosixFilePermissions(parent.toPath(), PosixFilePermissions.fromString("r-x------"))
        try {
            val unwritable = File(parent, "attachments")
            assertEquals(unwritable.path, assertFailsWith<PrivateDirectoryUnavailable> { writePrivateDesktopFile(unwritable, "a-view.txt") { it.write(1) } }.path)
        } finally { Files.setPosixFilePermissions(parent.toPath(), PosixFilePermissions.fromString("rwx------")) }
    }

    @Test fun viewerMessageNamesTheDirectoryAndSaysRetryingWillNotHelp() {
        val message = app.porchlight.shared.screens.collaboration.attachmentOpenError(
            PrivateDirectoryUnavailable("/home/me/.cache/porchlight/attachments", "it is a symbolic link"))
        assertTrue("/home/me/.cache/porchlight/attachments" in message, message)
        assertTrue("it is a symbolic link" in message, message)
        assertTrue("won't help" in message, message)
        assertFalse("Refetch" in message, message)
        for (transient in listOf(IllegalArgumentException("Attachment is no longer visible"), java.io.IOException("reset"), IllegalStateException()))
            assertEquals("Attachment could not open. Refetch or unlock before retrying.", app.porchlight.shared.screens.collaboration.attachmentOpenError(transient))
    }
}
