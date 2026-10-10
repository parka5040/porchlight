package app.porchlight.shared.files

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.provider.DocumentsContract
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import app.porchlight.shared.data.ExportSink
import java.io.File
import kotlinx.coroutines.*

/** SAF grants only the selected document; no storage permission or persisted broad access. */
fun calendarDocumentIntent(save: Boolean, household: Boolean = false): Intent = if (save)
    Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
        .setType(if (household) "application/gzip" else "application/json")
        .putExtra(Intent.EXTRA_TITLE, if (household) "porchlight-household.tar.gz" else "porchlight-member.json")
else Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
    .putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("text/calendar", "application/ics", "application/octet-stream", "text/plain"))

class CalendarDocumentActivity : Activity() {
    companion object { val requests = FileDialogSlot<Uri>() }
    private var owner: FileDialogSlot.Request<Uri>? = null
    @Suppress("DEPRECATION")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        owner = requests.current
        if (owner == null) { finish(); return }
        if (savedInstanceState == null) startActivityForResult(calendarDocumentIntent(intent.getBooleanExtra("save", false), intent.getBooleanExtra("household", false)), 1)
    }
    @Deprecated("Platform callback")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        owner?.let { requests.complete(it, data?.data?.takeIf { resultCode == RESULT_OK }) }
        finish()
    }
    override fun onDestroy() {
        if (!isChangingConfigurations) owner?.let(requests::cancel)
        super.onDestroy()
    }
}
@Composable actual fun rememberCalendarFiles(): CalendarFiles {
    val context = LocalContext.current.applicationContext
    val fallback = rememberCoroutineScope()
    val scope = app.porchlight.shared.screens.LocalScreenServices.current?.screenSession?.writeScope ?: fallback
    return remember(context, scope) { object : CalendarFiles {
        fun launch(save: Boolean, household: Boolean, callback: (Uri?) -> Unit): Boolean {
            val owner = CalendarDocumentActivity.requests.begin(callback) ?: return false
            try { context.startActivity(Intent(context, CalendarDocumentActivity::class.java).putExtra("save", save).putExtra("household", household).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
            catch (e: Exception) { CalendarDocumentActivity.requests.discard(owner); return false }
            return true
        }
        override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) {
            val launched = launch(false, false) { uri -> scope.launch {
                try {
                    val picked = uri?.let { withContext(Dispatchers.IO) {
                        val name = context.contentResolver.query(it, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null } ?: "calendar.ics"
                        val bytes = context.contentResolver.openInputStream(it)!!.use { stream ->
                            val out = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
                            while (true) { val n = stream.read(buffer); if (n < 0) break; require(out.size() + n <= 5 * 1024 * 1024); out.write(buffer, 0, n) }
                            out.toByteArray()
                        }
                        validateCalendarFile(name, bytes, context.contentResolver.getType(it))
                    } }
                    onResult(Result.success(picked))
                } catch (e: CancellationException) { throw e } catch (e: Exception) { onResult(Result.failure(e)) }
            } }
            if (!launched) onResult(Result.failure(IllegalStateException("A file dialog is already open or unavailable")))
        }
        override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) {
            val launched = launch(true, household) { uri -> scope.launch {
                if (uri == null) { onResult(Result.success(false)); return@launch }
                var sink: StagedExportSink? = null
                try {
                    val spool = withContext(Dispatchers.IO) { File.createTempFile("export-", ".part", context.cacheDir) }
                    val remove = { runCatching { DocumentsContract.deleteDocument(context.contentResolver, uri) }; Unit }
                    sink = StagedExportSink(spool, household, { context.contentResolver.openOutputStream(uri, "wt")!! }, remove)
                    write(sink); onResult(Result.success(true))
                } catch (e: CancellationException) {
                    withContext(NonCancellable) { sink?.abort() }
                    if (currentCoroutineContext().isActive) onResult(Result.failure(e)) else throw e
                }
                catch (e: Exception) { withContext(NonCancellable) { sink?.abort() }; onResult(Result.failure(e)) }
            } }
            if (!launched) onResult(Result.failure(IllegalStateException("A file dialog is already open or unavailable")))
        }
    } }
}
