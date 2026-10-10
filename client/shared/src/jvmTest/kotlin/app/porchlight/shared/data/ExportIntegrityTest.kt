package app.porchlight.shared.data

import app.porchlight.shared.files.*
import java.io.*
import java.util.zip.GZIPOutputStream
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class ExportIntegrityTest {
    private fun archive(marker: Boolean): ByteArray {
        val out = ByteArrayOutputStream()
        GZIPOutputStream(out).use { gz ->
            fun entry(name: String, body: String) {
                val bytes = body.toByteArray()
                val h = ByteArray(512)
                name.toByteArray().copyInto(h)
                String.format("%011o", bytes.size).toByteArray().copyInto(h, 124)
                h[156] = '0'.code.toByte()
                gz.write(h); gz.write(bytes); gz.write(ByteArray((512 - bytes.size % 512) % 512))
            }
            entry("data.json", "{\"format_version\":2}")
            if (marker) entry("complete.json", "{\"format_version\":2,\"complete\":true,\"table_rows\":{},\"blob_count\":0,\"blob_sha256\":[]}")
            gz.write(ByteArray(1024))
        }
        return out.toByteArray()
    }
    @Test fun failedDestinationOpenMustNotDeleteAnotherFile() = runTest {
        val dir = File(System.getProperty("porchlight.test.dir"), "exports").apply { mkdirs() }
        val temp = File.createTempFile("export-", ".part", dir)
        var removed = false
        val sink = StagedExportSink(temp, true, { throw java.nio.file.FileAlreadyExistsException("existing") }, { removed = true })
        try {
            sink.write(archive(true))
            assertFails { sink.finish() }
            sink.abort()
            assertFalse(removed, "Do not delete a destination that this export never opened")
        } finally { temp.delete() }
    }
    @Test fun householdRequiresFinalCompletionAndGzipTrailer() = runTest {
        val dir = File(System.getProperty("porchlight.test.dir"), "exports").apply { mkdirs() }
        for ((bytes, valid) in listOf(archive(true) to true, archive(false) to false, archive(true).dropLast(5).toByteArray() to false)) {
            val temp = File.createTempFile("export-", ".part", dir)
            var published = false
            val sink = StagedExportSink(temp, true, { published = true; ByteArrayOutputStream() }, {})
            try {
                sink.write(bytes)
                if (valid) sink.finish() else assertFails { sink.finish() }
                assertEquals(valid, published)
            } finally { sink.abort(); assertFalse(temp.exists()) }
        }
    }
    @Test fun invalidDownloadBeforePublishLeavesConfirmedOverwriteDestinationUntouched() = runTest {
        val dir = File(System.getProperty("porchlight.test.dir"), "exports").apply { mkdirs() }
        val spool = File.createTempFile("export-", ".part", dir)
        val existing = File.createTempFile("existing-", ".json", dir).apply { writeText("user data") }
        var opened = false
        val sink = StagedExportSink(spool, false, { opened = true; existing.outputStream() }, { existing.delete() })
        try {
            sink.write("incomplete".toByteArray())
            assertFails { sink.finish() }
            sink.abort()
            assertFalse(opened)
            assertEquals("user data", existing.readText())
            assertFalse(spool.exists())
        } finally { spool.delete(); existing.delete() }
    }

}
