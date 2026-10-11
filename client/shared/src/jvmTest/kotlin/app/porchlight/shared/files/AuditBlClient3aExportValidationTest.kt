package app.porchlight.shared.files

import java.io.File
import java.io.OutputStream
import java.nio.file.Files
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClient3aExportValidationTest {
    private fun directory() = Files.createTempDirectory(File(System.getProperty("porchlight.test.dir"), "export-audit").apply { mkdirs() }.toPath(), "io-").toFile()
    @Test fun largeMemberDocumentIsValidatedWithoutAWholeDocumentLimit() = runTest {
        val dir = directory(); val spool = File(dir, "spool.part")
        var published = 0L
        val sink = StagedExportSink(spool, false, { object : OutputStream() {
            override fun write(b: Int) { published++ }
            override fun write(b: ByteArray, off: Int, len: Int) { published += len }
        } }, {})
        try {
            sink.write("""{"format_version":1,"tables":{},"appointments_ics":"""".toByteArray())
            val chunk = ByteArray(64 * 1024) { 'a'.code.toByte() }
            repeat(1025) { sink.write(chunk) }
            sink.write("\"}".toByteArray())
            sink.finish()
            assertTrue(published > 64L * 1024 * 1024)
            assertFalse(spool.exists())
        } finally { sink.abort(); dir.deleteRecursively() }
    }
    @Test fun strictStreamingValidationRejectsMalformedDocumentsBeforePublication() = runTest {
        val dir = directory()
        try {
            val valid = """{"format_version":1,"tables":{},"appointments_ics":""}"""
            val invalid = listOf(valid + "x", valid.dropLast(1), valid.replace("1,", "01,"),
                valid.replace("\"\"}", "false}"), valid.replace("{}", "[]"),
                valid.replace("\"\"}", "\"bad\\q\"}"), valid.replace("\"\"}", "\"bad\n\"}"))
                .map { it.toByteArray() } + ("""{"format_version":1,"tables":{},"appointments_ics":"""".toByteArray() + byteArrayOf(0xc3.toByte()) + "\"}".toByteArray())
            for (bytes in invalid) {
                var opened = false
                val sink = StagedExportSink(File(dir, "spool.part"), false, { opened = true; OutputStream.nullOutputStream() }, {})
                try { sink.write(bytes); assertFails { sink.finish() }; assertFalse(opened) }
                finally { sink.abort() }
            }
        } finally { dir.deleteRecursively() }
    }
    @Test fun alreadyCreatedDocumentIsRemovedEvenBeforeSinkPublication() = runTest {
        val dir = directory()
        try {
            for (failure in listOf("spool", "validation", "cancel")) {
                var removed = 0
                var opened = false
                assertFails {
                    exportCreatedDocument(false, {
                        if (failure == "spool") error("Storage unavailable")
                        File.createTempFile("spool-", ".part", dir)
                    }, { opened = true; OutputStream.nullOutputStream() }, { removed++ }) {
                        if (failure == "cancel") throw CancellationException("Cancelled password dialog")
                        it.write("incomplete".toByteArray()); it.finish()
                    }
                }
                assertEquals(1, removed, "Created document leaked on $failure failure")
                assertFalse(opened)
                assertTrue(dir.listFiles().orEmpty().isEmpty())
            }
        } finally { dir.deleteRecursively() }
    }
    @Test fun validationChecksTheGateWhileReadingBeforeOpeningDestination() = runTest {
        val dir = directory()
        var checks = 0; var opened = false
        val sink = StagedExportSink(File(dir, "spool.part"), false, { opened = true; OutputStream.nullOutputStream() }, {})
        try {
            sink.write("""{"format_version":1,"tables":{},"appointments_ics":"""".toByteArray())
            sink.write(ByteArray(64 * 1024) { 'a'.code.toByte() }); sink.write("\"}".toByteArray())
            assertFailsWith<CancellationException> { sink.finish { if (++checks == 2) throw CancellationException("Locked") } }
            assertEquals(2, checks); assertFalse(opened)
        } finally { sink.abort(); dir.deleteRecursively() }
    }
    @OptIn(ExperimentalCoroutinesApi::class, DelicateCoroutinesApi::class)
    @Test fun cancelledPickerOwnerStillCleansAnAlreadyCreatedDocument() = runTest {
        var removed = 0
        val cancelled = CoroutineScope(Job().apply { cancel() } + Dispatchers.Default)
        cancelled.launch(start = CoroutineStart.ATOMIC) {
            exportCreatedDocument(false, { error("Must not create spool") }, { error("Must not publish") }, { removed++ }) { error("Must not download") }
        }.join()
        assertEquals(1, removed)
    }
    @Test fun successfulCreatedDocumentIsKept() = runTest {
        val dir = directory()
        try {
            var removed = false
            exportCreatedDocument(false, { File.createTempFile("spool-", ".part", dir) },
                { OutputStream.nullOutputStream() }, { removed = true }) {
                it.write("""{"format_version":1,"tables":{},"appointments_ics":""}""".toByteArray()); it.finish()
            }
            assertFalse(removed)
            assertTrue(dir.listFiles().orEmpty().isEmpty())
        } finally { dir.deleteRecursively() }
    }
}
