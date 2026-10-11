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
    return remember(scope) { DesktopCalendarFiles(scope, ::chooseCalendarFile) }
}

private suspend fun chooseCalendarFile(save: Boolean, household: Boolean): File? {
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

/** Picker and permission probe seams exercise the same callbacks as the real Swing picker. */
internal class DesktopCalendarFiles(
    private val scope: CoroutineScope,
    private val choose: suspend (save: Boolean, household: Boolean) -> File?,
    private val createSpool: () -> File = {
        val dir = File(System.getenv("XDG_DATA_HOME")?.takeIf { File(it).isAbsolute } ?: File(System.getProperty("user.home"), ".local/share").path, "porchlight/exports").apply { mkdirs() }
        File.createTempFile("export-", ".part", dir)
    },
    private val warning: (File) -> String? = { desktopExportWarning(it) },
) : CalendarFiles {
    override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { scope.launch {
        try {
            val file = choose(false, false)
            val picked = file?.let { withContext(Dispatchers.IO) {
                validateCalendarFile(it.name, readDesktopCalendar(it))
            } }
            onResult(Result.success(picked))
        } catch (e: CancellationException) { throw e }
        catch (e: UnsupportedCalendarArchitectureException) { onResult(Result.failure(e)) }
        catch (_: Exception) { onResult(Result.failure(IllegalStateException("Unable to read calendar. Choose a regular .ics file of at most 5 MiB."))) }
    } }
    override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) =
        save(household, write, {}, onResult)
    override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onWarning: (String) -> Unit, onResult: (Result<Boolean>) -> Unit) { scope.launch {
        var spool: File? = null
        var sink: StagedExportSink? = null
        try {
            val file = choose(true, household)
            if (file == null) { onResult(Result.success(false)); return@launch }
            require(!file.exists()) { "Choose a new filename; existing files are never overwritten" }
            // Keep ownership even if withContext discards its result on prompt cancellation.
            withContext(Dispatchers.IO) { spool = createSpool() }
            sink = StagedExportSink(checkNotNull(spool), household, { openDesktopExport(file) }, { file.delete() })
            write(sink)
            withContext(Dispatchers.IO) { warning(file) }?.let(onWarning)
            onResult(Result.success(true))
        } catch (e: CancellationException) {
            withContext(NonCancellable) { sink?.abort() }
            if (currentCoroutineContext().isActive) onResult(Result.failure(e)) else throw e
        } catch (_: Exception) {
            withContext(NonCancellable) { sink?.abort() }
            onResult(Result.failure(IllegalStateException("Unable to save export. Choose a new filename and check the destination.")))
        } finally {
            withContext(NonCancellable + Dispatchers.IO) { spool?.delete() }
        }
    } }
}
