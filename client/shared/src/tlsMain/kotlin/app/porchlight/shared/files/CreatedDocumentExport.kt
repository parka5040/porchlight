package app.porchlight.shared.files

import app.porchlight.shared.data.ExportSink
import java.io.File
import java.io.OutputStream
import kotlinx.coroutines.*

/** SAF has already created the destination before it gives us the URI, unlike CREATE_NEW on desktop. */
internal suspend fun exportCreatedDocument(household: Boolean, createSpool: () -> File,
    destination: () -> OutputStream, remove: () -> Unit, write: suspend (ExportSink) -> Unit) {
    var spool: File? = null
    var sink: StagedExportSink? = null
    var saved = false
    try {
        // Keep ownership even if withContext discards its result on prompt cancellation.
        withContext(Dispatchers.IO) { spool = createSpool() }
        sink = StagedExportSink(checkNotNull(spool), household, destination, {})
        write(sink)
        currentCoroutineContext().ensureActive()
        saved = true
    } finally {
        withContext(NonCancellable + Dispatchers.IO) {
            try { if (!saved) sink?.abort() }
            finally { spool?.delete(); if (!saved) remove() }
        }
    }
}
