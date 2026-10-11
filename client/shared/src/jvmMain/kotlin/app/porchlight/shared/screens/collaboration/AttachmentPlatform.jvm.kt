package app.porchlight.shared.screens.collaboration

import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.foundation.Image
import androidx.compose.ui.graphics.toComposeImageBitmap
import app.porchlight.shared.data.MAX_ATTACHMENT_BYTES
import kotlinx.coroutines.*
import kotlinx.coroutines.swing.Swing
import java.io.File
import java.nio.file.Files
import javax.swing.JFileChooser
import javax.swing.filechooser.FileNameExtensionFilter

@Composable actual fun rememberAttachmentActions(picks: AttachmentPicks, itemId: String, error: (String) -> Unit): AttachmentActions {
    // The modal chooser can outlive this composition (PIN relock): run it in the application scope, park the result.
    val scope = app.porchlight.shared.screens.applicationWriteScope()
    val onError by rememberUpdatedState(error)
    return remember(scope, picks, itemId) { object : AttachmentActions {
        private fun choose(photo: Boolean, commentId: String?) {
            val request = picks.begin(itemId, commentId)
            scope.launch {
                var value: PickedAttachment? = null
                var failure: String? = null
                try {
                    val f = withContext(Dispatchers.Swing) {
                        val chooser = JFileChooser().apply { fileFilter = if (photo) FileNameExtensionFilter("Images", "jpg", "jpeg", "png", "webp", "gif") else FileNameExtensionFilter("Images, PDF or text", "jpg", "jpeg", "png", "webp", "gif", "pdf", "txt") }
                        if (chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION) chooser.selectedFile else null
                    }
                    if (f != null) value = withContext(Dispatchers.IO) {
                        if (f.length() > MAX_ATTACHMENT_BYTES) throw app.porchlight.shared.data.AttachmentTooLarge()
                        val bytes = f.inputStream().use { it.readNBytes(MAX_ATTACHMENT_BYTES + 1) }
                        if (bytes.size > MAX_ATTACHMENT_BYTES) throw app.porchlight.shared.data.AttachmentTooLarge()
                        PickedAttachment(f.name, Files.probeContentType(f.toPath()) ?: attachmentMimeFromName(f.name) ?: throw IllegalArgumentException("Unrecognized attachment type"), bytes)
                    }
                } catch (e: CancellationException) { throw e } catch (_: app.porchlight.shared.data.AttachmentTooLarge) { failure = "Attachments must be at most 25 MiB." } catch (_: Exception) { failure = "Could not read selected file." }
                finally { picks.complete(request.id, value, failure) }
            }
        }
        override fun photo(commentId: String?) = choose(true, commentId)
        override fun file(commentId: String?) = choose(false, commentId)
        override fun camera(commentId: String?) { onError("Camera capture is available on Android. Choose a photo on desktop.") }
        override suspend fun prepareViewer(id: String, mime: String, bytes: ByteArray): app.porchlight.shared.data.PreparedAttachmentViewer {
            var prepared: File? = null
            try {
                val file = withContext(Dispatchers.IO) {
                    require(id.matches(Regex("[a-zA-Z0-9_-]+")))
                    val root = File(System.getenv("XDG_CACHE_HOME")?.takeIf { File(it).isAbsolute } ?: File(System.getProperty("user.home"), ".cache").path, "porchlight/attachments")
                    app.porchlight.shared.files.writePrivateDesktopFile(root, "$id-view.${java.util.UUID.randomUUID()}.${attachmentExtension(mime)}") { it.write(bytes) }.also { prepared = it }
                }
                return object : app.porchlight.shared.data.PreparedAttachmentViewer {
                    override suspend fun launch() = withContext(Dispatchers.IO) {
                        currentCoroutineContext().ensureActive()
                        ProcessBuilder("xdg-open", file.absolutePath).redirectError(ProcessBuilder.Redirect.DISCARD).redirectOutput(ProcessBuilder.Redirect.DISCARD).start()
                        Unit
                    }
                    override suspend fun discard() { withContext(Dispatchers.IO) { check(!file.exists() || file.delete()) } }
                }
            } catch (e: Exception) {
                withContext(NonCancellable + Dispatchers.IO) { prepared?.delete() }
                throw e
            }
        }
    } }
}
@Composable actual fun AttachmentThumbnail(bytes: ByteArray, description: String, modifier: Modifier) {
    val image by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, bytes) {
        value = withContext(Dispatchers.Default) { runCatching { javax.imageio.ImageIO.read(bytes.inputStream())?.toComposeImageBitmap() }.getOrNull() }
    }
    image?.let { Image(it, description, modifier) }
}
