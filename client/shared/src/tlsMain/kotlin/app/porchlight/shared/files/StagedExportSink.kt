package app.porchlight.shared.files

import app.porchlight.shared.data.ExportSink
import app.porchlight.shared.transport.WireJson
import java.io.*
import java.util.zip.GZIPInputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*

/** Private spool, bounded archive validation, then publish to the user's chosen destination. */
class StagedExportSink(private val spool: File, private val household: Boolean,
    private val destination: () -> OutputStream, private val removeDestination: () -> Unit) : ExportSink {
    private var output: OutputStream? = null
    private var publishing = false
    override suspend fun write(bytes: ByteArray) = withContext(Dispatchers.IO) {
        if (output == null) {
            spool.setReadable(false, false); spool.setWritable(false, false)
            spool.setReadable(true, true); spool.setWritable(true, true)
            output = spool.outputStream().buffered()
        }
        output!!.write(bytes)
    }
    override suspend fun finish(checkGate: () -> Unit) = withContext(Dispatchers.IO) {
        output?.close(); output = null
        if (household) validateHouseholdArchive(spool) else validateMemberExport(spool, checkGate)
        checkGate()
        val target = destination()
        publishing = true
        target.use { dest -> spool.inputStream().use { source ->
            val bytes = ByteArray(64 * 1024)
            while (true) {
                val count = source.read(bytes); if (count < 0) break
                checkGate(); dest.write(bytes, 0, count)
            }
        } }
        checkGate()
        spool.delete()
        Unit
    }
    override suspend fun abort() = withContext(Dispatchers.IO) {
        output?.close(); output = null
        spool.delete()
        if (publishing) removeDestination()
    }
}
fun validateHouseholdArchive(file: File) {
    GZIPInputStream(file.inputStream().buffered()).use { input ->
        val header = ByteArray(512)
        var last = ""
        var marker: JsonObject? = null
        val blobs = mutableSetOf<String>()
        val tables = mutableSetOf<String>()
        fun exact(bytes: ByteArray) { var offset = 0; while (offset < bytes.size) { val n = input.read(bytes, offset, bytes.size - offset); require(n > 0) { "Incomplete export" }; offset += n } }
        fun skip(count: Long) { var left = count; val buffer = ByteArray(64 * 1024); while (left > 0) { val n = input.read(buffer, 0, minOf(buffer.size.toLong(), left).toInt()); require(n > 0) { "Incomplete export" }; left -= n } }
        while (true) {
            exact(header)
            if (header.all { it == 0.toByte() }) {
                exact(header); require(header.all { it == 0.toByte() })
                // Drain to EOF: GZIPInputStream checks the CRC/trailer here, including truncated streams.
                val buffer = ByteArray(8192)
                while (true) { val n = input.read(buffer); if (n < 0) break; require(buffer.take(n).all { it == 0.toByte() }) }
                break
            }
            require(marker == null) { "Completion marker must be last" }
            val name = header.copyOfRange(0, 100).toString(Charsets.UTF_8).substringBefore('\u0000')
            val size = header.copyOfRange(124, 136).toString(Charsets.US_ASCII).trim('\u0000', ' ').toLong(8)
            require(size >= 0 && header[156].toInt().toChar() in listOf('0', '\u0000')) { "Unsupported export entry" }
            last = name
            if (name == "complete.json") {
                require(size <= 16L * 1024 * 1024)
                val bytes = ByteArray(size.toInt()); exact(bytes)
                marker = WireJson.parseToJsonElement(bytes.decodeToString()).jsonObject
            } else {
                if (name.startsWith("blobs/")) blobs += name.substringAfterLast('/')
                if (name.startsWith("tables/") && name.endsWith(".jsonl")) tables += name.removePrefix("tables/").removeSuffix(".jsonl")
                skip(size)
            }
            skip((512 - size % 512) % 512)
        }
        val complete = requireNotNull(marker) { "Missing completion marker" }
        require(last == "complete.json" && complete["complete"]?.jsonPrimitive?.boolean == true && complete["format_version"]?.jsonPrimitive?.int == 2)
        require(complete["blob_count"]?.jsonPrimitive?.int == blobs.size)
        require(complete["blob_sha256"]?.jsonArray?.map { it.jsonPrimitive.content }?.toSet() == blobs)
        require(complete["table_rows"]?.jsonObject?.keys == tables)
    }
}
