package app.porchlight.shared.screens.collaboration

import app.porchlight.shared.data.AttachmentContent
import app.porchlight.shared.data.PreparedAttachmentViewer
import app.porchlight.shared.files.writePrivateDesktopStream
import kotlinx.coroutines.*
import java.nio.file.Path
import java.nio.file.Files

/** Never sniff attachment bytes: explicitly launch the server MIME's default handler, preferring gio. */
internal fun desktopAttachmentViewerCommand(path: String, mime: String, query: (String, String) -> String,
    handler: (String) -> Path?, gioAvailable: Boolean = true): List<String> {
    require(attachmentExtension(mime) != "bin")
    val desktop = query("default", mime)
    require(desktop.matches(Regex("[a-zA-Z0-9_.-]+\\.desktop"))) { "No safe attachment viewer" }
    val entry = requireNotNull(handler(desktop)) { "No attachment MIME handler" }
    if (gioAvailable) return listOf("gio", "launch", entry.toString(), path)
    return desktopEntryCommand(readDesktopEntry(entry), path, entry.toString())
}
private fun openDesktopAttachment(path: String, mime: String) {
    val command = desktopAttachmentViewerCommand(path, mime, query = { kind, value ->
        val process = ProcessBuilder("xdg-mime", "query", kind, value).redirectError(ProcessBuilder.Redirect.DISCARD).start()
        try {
            check(process.waitFor(2, java.util.concurrent.TimeUnit.SECONDS) && process.exitValue() == 0) { "Attachment MIME lookup failed" }
            process.inputStream.use { it.readNBytes(1024).toString(Charsets.UTF_8).trim() }
        } finally { if (process.isAlive) process.destroyForcibly() }
    }, handler = { desktop ->
        val dataHome = System.getenv("XDG_DATA_HOME")?.takeIf { Path.of(it).isAbsolute }
            ?: Path.of(System.getProperty("user.home"), ".local/share").toString()
        desktopAttachmentHandler(desktop, dataHome, System.getenv("XDG_DATA_DIRS") ?: "/usr/local/share:/usr/share")
    }, gioAvailable = System.getenv("PATH").orEmpty().split(':').any { directory ->
        directory.isNotBlank() && Path.of(directory).isAbsolute &&
            Path.of(directory, "gio").let { Files.isRegularFile(it) && Files.isExecutable(it) }
    })
    // No xdg-open fallback after an explicit-handler failure: that could promote text to active content.
    ProcessBuilder(command).redirectError(ProcessBuilder.Redirect.DISCARD).redirectOutput(ProcessBuilder.Redirect.DISCARD).start()
}

internal suspend fun prepareDesktopAttachmentViewer(root: Path, id: String, mime: String, content: AttachmentContent,
    open: (String, String) -> Unit = ::openDesktopAttachment): PreparedAttachmentViewer {
    val extension = attachmentExtension(mime)
    require(extension != "bin") { "Unsupported attachment MIME" }
    // The alias carries the authoritative server type and outlives LRU eviction of the original. It remains
    // identity/visibility-owned via the existing id-view cleanup, not owned by the external process.
    var prepared: Path? = null
    try {
        val file = withContext(Dispatchers.IO) {
            require(id.matches(Regex("[a-zA-Z0-9_-]+")))
            writePrivateDesktopStream(root, "$id-view.${java.util.UUID.randomUUID()}.$extension") { output -> content.chunks { output.write(it) } }.also { prepared = it }
        }
        return object : PreparedAttachmentViewer {
            override suspend fun launch() = withContext(Dispatchers.IO) { currentCoroutineContext().ensureActive(); open(file.toString(), mime) }
            override suspend fun discard() { withContext(Dispatchers.IO) { Files.deleteIfExists(file); Unit } }
        }
    } catch (e: Exception) {
        withContext(NonCancellable + Dispatchers.IO) { prepared?.let { Files.deleteIfExists(it) } }
        throw e
    }
}
