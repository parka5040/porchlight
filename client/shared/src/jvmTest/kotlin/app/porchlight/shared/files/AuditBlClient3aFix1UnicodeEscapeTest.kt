package app.porchlight.shared.files

import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClient3aFix1UnicodeEscapeTest {
    private fun directory() = Files.createTempDirectory(
        File(System.getProperty("porchlight.test.dir"), "export-escape-audit").apply { mkdirs() }.toPath(), "io-").toFile()

    private suspend fun rejectsBeforePublication(escape: String) {
        val dir = directory()
        var opened = false
        val spool = File(dir, "spool.part")
        val sink = StagedExportSink(spool, false, { opened = true; ByteArrayOutputStream() }, {})
        try {
            sink.write("""{"format_version":1,"tables":{},"appointments_ics":"$escape"}""".toByteArray())
            assertFailsWith<IllegalArgumentException>("Non-ASCII Unicode escape digits must be rejected") { sink.finish() }
            assertFalse(opened, "Invalid JSON must not open the destination")
        } finally { sink.abort(); dir.deleteRecursively() }
    }

    @Test fun arabicIndicEscapeDigitsAreRejectedBeforeOpeningDestination() = runTest {
        rejectsBeforePublication("""\u١٢٣٤""")
    }

    @Test fun fullwidthEscapeDigitsAreRejectedBeforeOpeningDestination() = runTest {
        rejectsBeforePublication("""\u１２Ａｆ""")
    }

    @Test fun asciiEscapedUnicodeRemainsValidAndIsPublishedUnchanged() = runTest {
        val dir = directory()
        val spool = File(dir, "spool.part")
        val destination = ByteArrayOutputStream()
        var opened = 0
        val sink = StagedExportSink(spool, false, { opened++; destination }, {})
        val escapes = listOf("000a", "00aF", "00Af", "0041", "D83D", "DE00").joinToString("") { "\\u$it" }
        val bytes = """{"format_version":1,"tables":{},"appointments_ics":"$escapes"}""".toByteArray()
        try {
            sink.write(bytes); sink.finish()
            assertEquals(1, opened)
            assertContentEquals(bytes, destination.toByteArray())
            assertFalse(spool.exists())
        } finally { sink.abort(); dir.deleteRecursively() }
    }
}
