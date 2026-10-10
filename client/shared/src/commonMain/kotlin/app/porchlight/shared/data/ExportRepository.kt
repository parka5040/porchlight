package app.porchlight.shared.data

import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

/** A staged destination: finish validates completeness before publishing; abort removes all partial data. */
interface ExportSink {
    suspend fun write(bytes: ByteArray)
    suspend fun finish(checkGate: () -> Unit = {})
    suspend fun abort()
}
class ExportRepository(private val call: GeneratedCall, private val cache: ReadCache? = null) {
    suspend fun member(sink: ExportSink) = save("/v1/export/member", null, sink)
    suspend fun household(stepUp: String, sink: ExportSink) = save("/v1/admin/export/household", stepUp, sink)
    private suspend fun save(path: String, stepUp: String?, sink: ExportSink) {
        try {
            val op: suspend () -> Unit = { call.download(path, stepUp, sink::write) { checkGate -> sink.finish(checkGate) } }
            if (cache == null) op() else cache.inSession { op() }
        } catch (e: Exception) {
            withContext(NonCancellable) { sink.abort() }
            throw e
        }
    }
}
