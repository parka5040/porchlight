package app.porchlight.shared.files

import java.io.File
import java.io.InputStream
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class DesktopCalendarIoTest {
    private val privateMode = PosixFilePermissions.fromString("rw-------")
    private val limit = 5 * 1024 * 1024
    private fun directory() = Files.createTempDirectory(
        File(System.getProperty("porchlight.test.dir"), "desktop-calendar").apply { mkdirs() }.toPath(), "io-"
    ).toFile()

    @Test fun exportIsPrivateFromOpenThroughPublication() = runTest {
        val dir = directory()
        val target = File(dir, "member.json")
        val spool = File.createTempFile("spool-", ".part", dir)
        var permissionsAtOpen: Set<java.nio.file.attribute.PosixFilePermission>? = null
        val sink = StagedExportSink(spool, false, {
            openDesktopExport(target).also { permissionsAtOpen = Files.getPosixFilePermissions(target.toPath()) }
        }, { target.delete() })
        try {
            sink.write("{\"format_version\":1,\"tables\":{},\"appointments_ics\":\"BEGIN:VCALENDAR\"}".toByteArray())
            sink.finish()
            assertEquals(privateMode, permissionsAtOpen, "Private at creation, not chmod after copying")
            assertEquals(privateMode, Files.getPosixFilePermissions(target.toPath()))
            assertTrue(target.readText().contains("BEGIN:VCALENDAR"))
            assertFalse(spool.exists())
        } finally { sink.abort(); dir.deleteRecursively() }
    }

    @Test fun existingPublicDestinationIsNeverOverwrittenOrDeleted() = runTest {
        val dir = directory()
        val target = File(dir, "existing.json").apply { writeText("existing user data") }
        val publicMode = PosixFilePermissions.fromString("rw-rw-rw-")
        Files.setPosixFilePermissions(target.toPath(), publicMode)
        val spool = File.createTempFile("spool-", ".part", dir)
        val sink = StagedExportSink(spool, false, { openDesktopExport(target) }, { target.delete() })
        try {
            sink.write("{\"format_version\":1,\"tables\":{},\"appointments_ics\":\"\"}".toByteArray())
            assertFailsWith<FileAlreadyExistsException> { sink.finish() }
            sink.abort()
            assertEquals("existing user data", target.readText())
            assertEquals(publicMode, Files.getPosixFilePermissions(target.toPath()))
            val link = File(dir, "link.json")
            Files.createSymbolicLink(link.toPath(), target.toPath())
            assertFailsWith<FileAlreadyExistsException> { openDesktopExport(link) }
            assertEquals("existing user data", target.readText())
        } finally { sink.abort(); dir.deleteRecursively() }
    }

    @Test fun privateExportHasNoWarning() {
        val dir = directory()
        try {
            val target = File(dir, "private.json")
            openDesktopExport(target).use { it.write(1) }
            assertNull(desktopExportWarning(target))
        } finally { dir.deleteRecursively() }
    }

    @Test fun filesystemIgnoringRequestedModeKeepsFileWithWarning() {
        val dir = directory()
        try {
            val target = File(dir, "stick.json")
            openDesktopExport(target).use { it.write(1) }
            val mount = PosixFilePermissions.fromString("rw-r--r--") // what vfat/exFAT report
            assertEquals(EXPORT_NOT_PRIVATE_WARNING, desktopExportWarning(target) { mount })
            assertTrue(target.exists(), "Export is kept, not deleted")
            val worldOnly = PosixFilePermissions.fromString("rw----r--")
            assertEquals(EXPORT_NOT_PRIVATE_WARNING, desktopExportWarning(target) { worldOnly })
        } finally { dir.deleteRecursively() }
    }

    @Test fun unsupportedOrUnreadablePermissionsCannotConfirmPrivacy() {
        val file = File("unused")
        assertEquals(EXPORT_NOT_PRIVATE_WARNING, desktopExportWarning(file) { null })
        assertEquals(EXPORT_NOT_PRIVATE_WARNING, desktopExportWarning(file) { throw java.io.IOException("stat") })
    }

    /** Size-zero/FIFO-like source: no stat, produces twice the limit without a large fixture. */
    private class GrowingStream(private val total: Int) : InputStream() {
        var consumed = 0
        override fun read(): Int = if (consumed == total) -1 else { consumed++; 'x'.code }
        override fun read(bytes: ByteArray, offset: Int, length: Int): Int {
            if (consumed == total) return -1
            val count = minOf(length, total - consumed)
            bytes.fill('x'.code.toByte(), offset, offset + count)
            consumed += count
            return count
        }
    }

    @Test fun oversizedStreamReadsAtMostLimitPlusOne() {
        val source = GrowingStream(2 * limit)
        assertFailsWith<IllegalArgumentException> { readDesktopCalendar(source) }
        assertEquals(limit + 1, source.consumed, "Do not allocate/read the remainder before rejecting")
    }

    @Test fun exactLimitIsAcceptedAndSmallReadsArePreserved() {
        val exact = GrowingStream(limit)
        assertEquals(limit, readDesktopCalendar(exact).size)
        assertEquals(limit, exact.consumed)
        val calendar = "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".toByteArray()
        assertContentEquals(calendar, readDesktopCalendar(calendar.inputStream()))
        val fragmented = object : java.io.ByteArrayInputStream(calendar) {
            override fun read(bytes: ByteArray, offset: Int, length: Int): Int = super.read(bytes, offset, minOf(length, 3))
        }
        assertContentEquals(calendar, readDesktopCalendar(fragmented))
        assertTrue(readDesktopCalendar(byteArrayOf().inputStream()).isEmpty())
    }
}
