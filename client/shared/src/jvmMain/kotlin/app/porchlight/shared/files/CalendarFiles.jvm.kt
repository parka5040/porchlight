package app.porchlight.shared.files

import androidx.compose.runtime.*
import app.porchlight.shared.data.ExportSink
import java.io.File
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter
import java.awt.EventQueue
import kotlinx.coroutines.*

@Composable actual fun rememberCalendarFiles(): CalendarFiles {
    val fallback = rememberCoroutineScope()
    val scope = app.porchlight.shared.screens.LocalScreenServices.current?.screenSession?.writeScope ?: fallback
    return remember(scope) { object : CalendarFiles {
        suspend fun choose(save: Boolean, household: Boolean = false): File? {
            val result = CompletableDeferred<File?>()
            EventQueue.invokeLater {
                try {
                    val chooser = JFileChooser()
                    if (save) chooser.selectedFile = File(if (household) "porchlight-household.tar.gz" else "porchlight-member.json")
                    else chooser.fileFilter = FileNameExtensionFilter("ICS calendars", "ics")
                    val selected = if (save) chooser.showSaveDialog(null) else chooser.showOpenDialog(null)
                    result.complete(if (selected == JFileChooser.APPROVE_OPTION) chooser.selectedFile else null)
                } catch (e: Exception) { result.completeExceptionally(e) }
            }
            return result.await()
        }
        override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { scope.launch {
            try {
                val file = choose(false)
                val picked = file?.let { withContext(Dispatchers.IO) {
                    validateCalendarFile(it.name, it.inputStream().use(::readDesktopCalendar))
                } }
                onResult(Result.success(picked))
            } catch (e: CancellationException) { throw e } catch (e: Exception) { onResult(Result.failure(e)) }
        } }
        override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) =
            save(household, write, {}, onResult)
        override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onWarning: (String) -> Unit, onResult: (Result<Boolean>) -> Unit) { scope.launch {
            var sink: StagedExportSink? = null
            try {
                val file = choose(true, household)
                if (file == null) { onResult(Result.success(false)); return@launch }
                require(!file.exists()) { "Choose a new filename; existing files are never overwritten" }
                val spoolDir = File(System.getenv("XDG_DATA_HOME")?.takeIf { File(it).isAbsolute } ?: File(System.getProperty("user.home"), ".local/share").path, "porchlight/exports").apply { mkdirs() }
                val spool = withContext(Dispatchers.IO) { File.createTempFile("export-", ".part", spoolDir) }
                sink = StagedExportSink(spool, household, { openDesktopExport(file) }, { file.delete() })
                write(sink)
                withContext(Dispatchers.IO) { desktopExportWarning(file) }?.let(onWarning)
                onResult(Result.success(true))
            } catch (e: CancellationException) {
                    withContext(NonCancellable) { sink?.abort() }
                    if (currentCoroutineContext().isActive) onResult(Result.failure(e)) else throw e
                }
            catch (e: Exception) { sink?.abort(); onResult(Result.failure(e)) }
        } }
    } }
}
