package app.porchlight.shared.files

import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.attribute.PosixFilePermissions
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClient3aDesktopFilesTest {
    private fun directory() = Files.createTempDirectory(
        File(System.getProperty("porchlight.test.dir"), "calendar-audit").apply { mkdirs() }.toPath(), "io-").toFile()
    private val json = """{"format_version":1,"tables":{},"appointments_ics":""}""".toByteArray()

    private fun child(action: String, file: File): Process {
        val java = File(System.getProperty("java.home"), "bin/java").path
        val cp = System.getProperty("java.class.path")
        return ProcessBuilder("bash", "-c", "umask 000; exec \"\$@\"", "calendar-test", java,
            "--enable-native-access=ALL-UNNAMED", "-cp", cp, AuditBlClient3aFileChild::class.java.name, action, file.path)
            .redirectErrorStream(true).start()
    }
    @Test fun fifoIsRejectedWithoutWaitingForAWriter() {
        val dir = directory()
        try {
            val fifo = File(dir, "pipe.ics")
            val mkfifo = ProcessBuilder("mkfifo", fifo.path).start()
            try { assertTrue(mkfifo.waitFor(5, TimeUnit.SECONDS)); assertEquals(0, mkfifo.exitValue()) }
            finally { if (mkfifo.isAlive) { mkfifo.destroyForcibly(); mkfifo.waitFor() } }
            val process = child("reject", fifo)
            try {
                assertTrue(process.waitFor(5, TimeUnit.SECONDS), "FIFO open blocked waiting for a writer")
                assertEquals(0, process.exitValue(), process.inputStream.bufferedReader().readText())
            } finally { if (process.isAlive) { process.destroyForcibly(); process.waitFor() } }
        } finally { dir.deleteRecursively() }
    }
    @Test fun importRejectsSymlinksAndDirectoriesButReadsRegularFiles() {
        val dir = directory()
        try {
            val file = File(dir, "calendar.ics").apply { writeText("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n") }
            assertContentEquals(file.readBytes(), readDesktopCalendar(file))
            val link = File(dir, "alias.ics")
            Files.createSymbolicLink(link.toPath(), file.toPath())
            assertFails { readDesktopCalendar(link) }
            assertFails { readDesktopCalendar(dir) }
        } finally { dir.deleteRecursively() }
    }
    @Test fun exportIsPrivateEvenUnderPermissiveSubprocessUmask() {
        val dir = directory()
        try {
            val target = File(dir, "member.json")
            val process = child("private", target)
            try {
                assertTrue(process.waitFor(10, TimeUnit.SECONDS))
                assertEquals(0, process.exitValue(), process.inputStream.bufferedReader().readText())
                assertEquals(PosixFilePermissions.fromString("rw-------"), Files.getPosixFilePermissions(target.toPath()))
            } finally { if (process.isAlive) { process.destroyForcibly(); process.waitFor() } }
        } finally { dir.deleteRecursively() }
    }
    @Test fun danglingSymlinkIsPreservedAndPickerFailureIsSanitized() = runTest {
        val dir = directory()
        try {
            val target = File(dir, "absent.json")
            val link = File(dir, "alias.json")
            Files.createSymbolicLink(link.toPath(), target.toPath())
            val files = DesktopCalendarFiles(this, { _, _ -> link }, { File.createTempFile("spool-", ".part", dir) })
            val result = CompletableDeferred<Result<Boolean>>()
            files.save(false, { it.write(json); it.finish() }, { result.complete(it) })
            val failure = result.await().exceptionOrNull()
            assertNotNull(failure)
            assertFalse(failure.message.orEmpty().contains(dir.path), "Picker failure leaked destination path")
            assertTrue(Files.exists(link.toPath(), LinkOption.NOFOLLOW_LINKS))
            assertFalse(target.exists())
        } finally { dir.deleteRecursively() }
    }
    @Test fun pickerCancellationAndWarningUseProductionCallbacksInOrder() = runTest {
        val dir = directory()
        try {
            val cancelled = CompletableDeferred<Result<Boolean>>()
            DesktopCalendarFiles(this, { _, _ -> null }).save(true, { fail("Cancelled picker must not start export") }, { cancelled.complete(it) })
            assertEquals(false, cancelled.await().getOrThrow())
            val target = File(dir, "member.json")
            val events = mutableListOf<String>()
            val result = CompletableDeferred<Result<Boolean>>()
            val files = DesktopCalendarFiles(this, { save, household ->
                assertTrue(save); assertFalse(household); events += "picked"; target
            }, { File.createTempFile("spool-", ".part", dir) }, { file ->
                assertContentEquals(json, file.readBytes())
                desktopExportWarning(file) { PosixFilePermissions.fromString("rw-rw-rw-") }
            })
            files.save(false, { events += "write"; it.write(json); it.finish() }, {
                assertEquals(EXPORT_NOT_PRIVATE_WARNING, it); events += "warning"
            }, { events += "result"; result.complete(it) })
            assertTrue(result.await().getOrThrow())
            assertEquals(listOf("picked", "write", "warning", "result"), events)
            assertContentEquals(json, target.readBytes())
        } finally { dir.deleteRecursively() }
    }
}

object AuditBlClient3aFileChild {
    @JvmStatic fun main(args: Array<String>) {
        val file = File(args[1])
        when (args[0]) {
            "reject" -> assertFails { readDesktopCalendar(file) }
            "private" -> openDesktopExport(file).use {
                assertEquals(PosixFilePermissions.fromString("rw-------"), Files.getPosixFilePermissions(file.toPath()))
                it.write(1)
            }
        }
    }
}
