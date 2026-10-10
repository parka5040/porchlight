package app.porchlight.shared.screens.collaboration

import android.app.Activity
import android.content.*
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.MediaStore
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.Image
import androidx.compose.ui.graphics.asImageBitmap
import app.porchlight.shared.data.MAX_ATTACHMENT_BYTES
import app.porchlight.shared.data.AttachmentTooLarge
import kotlinx.coroutines.*
import java.io.File

fun photoPickerIntent(): Intent = if (android.os.Build.VERSION.SDK_INT >= 33) Intent(MediaStore.ACTION_PICK_IMAGES).setType("image/*")
    else Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("image/*")
fun filePickerIntent(): Intent = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*").putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("image/*", "application/pdf", "text/plain"))
fun cameraIntent(uri: Uri): Intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE).putExtra(MediaStore.EXTRA_OUTPUT, uri)
    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION).apply { clipData = ClipData.newRawUri("Photo output", uri) }
/**
 * C5-06: process-level route from the helper Activity back to the identity-scoped [AttachmentPicks] that launched it.
 * It is deliberately NOT tied to the launching composition, which rotation or the PIN relock (ON_STOP, triggered by
 * every full-screen picker/camera) discards while the picker is open. Memory only: after process death the result
 * is dropped (the captured photo is still deleted), like every other session draft.
 */
internal object AttachmentPickRegistry {
    private val inboxes = java.util.concurrent.ConcurrentHashMap<String, AttachmentPicks>()
    fun register(picks: AttachmentPicks, request: AttachmentPicks.Request) { inboxes[request.id] = picks }
    /** Delivers on the main thread. False when nothing waits for [id] (completed, or its identity was cleared). */
    fun complete(id: String?, value: PickedAttachment?, error: String?): Boolean =
        id != null && inboxes.remove(id)?.complete(id, value, error) == true
}
@Composable actual fun rememberAttachmentActions(picks: AttachmentPicks, itemId: String, error: (String) -> Unit): AttachmentActions {
    val context = LocalContext.current
    val onError by rememberUpdatedState(error)
    return remember(context, picks, itemId) { object : AttachmentActions {
        private fun pick(mode: String, commentId: String?) {
            val request = picks.begin(itemId, commentId)
            AttachmentPickRegistry.register(picks, request)
            try { context.startActivity(Intent(context, AttachmentPickerActivity::class.java).putExtra("request", request.id).putExtra("mode", mode)) }
            catch (_: Exception) { AttachmentPickRegistry.complete(request.id, null, null); onError("No picker or camera is available.") }
        }
        override fun photo(commentId: String?) = pick("photo", commentId)
        override fun file(commentId: String?) = pick("file", commentId)
        override fun camera(commentId: String?) = pick("camera", commentId)
        override suspend fun open(id: String, mime: String, bytes: ByteArray) {
            val file = withContext(Dispatchers.IO) {
                require(id.matches(Regex("[a-zA-Z0-9_-]+")))
                val root = File(context.cacheDir, "attachments"); root.mkdirs()
                File(root, "$id-view.${attachmentExtension(mime)}").also { it.writeBytes(bytes); it.setReadable(false, false); it.setWritable(false, false); it.setReadable(true, true); it.setWritable(true, true) }
            }
            val uri = FileProvider.getUriForFile(context, context.packageName + ".attachments", file)
            context.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION).apply { clipData = ClipData.newRawUri("Attachment", uri) })
        }
    } }
}
@Composable actual fun AttachmentThumbnail(bytes: ByteArray, description: String, modifier: Modifier) {
    val bitmap by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, bytes) {
        value = withContext(Dispatchers.Default) { runCatching { android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() }.getOrNull() }
    }
    bitmap?.let { Image(it, description, modifier) }
}
/**
 * Transparent bridge: only app-owned request IDs; no grants persist and results are read with a strict byte cap.
 * The result goes to [AttachmentPickRegistry], never to a composition, so it survives MainActivity recreation and
 * the PIN relock; the gated item screen uploads it after unlock. The captured photo is deleted once read.
 */
class AttachmentPickerActivity : Activity() {
    private var cameraFile: File? = null
    private var completed = false
    private val work = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        cameraFile = savedInstanceState?.getString("camera_file")?.let(::File)
        if (savedInstanceState == null) {
            if (intent.getStringExtra("mode") == "camera" && checkSelfPermission(android.Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf(android.Manifest.permission.CAMERA), 12)
            else launchPicker()
        }
    }
    override fun onSaveInstanceState(outState: Bundle) { cameraFile?.let { outState.putString("camera_file", it.path) }; super.onSaveInstanceState(outState) }
    private fun launchPicker() {
        try {
            val request = when (intent.getStringExtra("mode")) {
                "photo" -> photoPickerIntent()
                "camera" -> {
                    val root = File(cacheDir, "attachments"); root.mkdirs()
                    val f = File.createTempFile("capture-", ".jpg", root); cameraFile = f
                    cameraIntent(FileProvider.getUriForFile(this, packageName + ".attachments", f))
                }
                else -> filePickerIntent()
            }
            @Suppress("DEPRECATION")
            startActivityForResult(request, 11)
        } catch (_: Exception) { complete(null, "No picker or camera is available.") }
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 12 && grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) launchPicker() else complete(null, "Camera permission was not granted.")
    }
    @Deprecated("Bridge uses Activity results")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != 11 || resultCode != RESULT_OK) { complete(null, null); return }
        val uri = data?.data ?: cameraFile?.let { FileProvider.getUriForFile(this, packageName + ".attachments", it) }
        if (uri == null) { complete(null, "No file selected."); return }
        work.launch {
            try {
                val value = withContext(Dispatchers.IO) {
                    var name = "photo.jpg"; var size: Long? = null
                    contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                        if (c.moveToFirst()) { name = c.getString(0) ?: name; if (!c.isNull(1)) size = c.getLong(1) }
                    }
                    if ((size ?: 0) > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
                    val bytes = contentResolver.openInputStream(uri)?.use { stream ->
                        val buffer = ByteArray(MAX_ATTACHMENT_BYTES + 1); var count = 0
                        while (count < buffer.size) { val n = stream.read(buffer, count, buffer.size - count); if (n < 0) break; count += n }
                        buffer.copyOf(count) } ?: error("No stream")
                    if (bytes.size > MAX_ATTACHMENT_BYTES) throw AttachmentTooLarge()
                    PickedAttachment(name, contentResolver.getType(uri) ?: "image/jpeg", bytes)
                }
                complete(value, null)
            } catch (e: CancellationException) { throw e } catch (_: AttachmentTooLarge) { complete(null, "Attachments must be at most 25 MiB.") } catch (_: Exception) { complete(null, "Could not read selected attachment.") }
        }
    }
    private fun complete(value: PickedAttachment?, error: String?) {
        completed = true
        AttachmentPickRegistry.complete(intent.getStringExtra("request"), value, error); cameraFile?.delete(); finish()
    }
    override fun onDestroy() {
        // Destroyed while waiting but not by a configuration change or the system reclaiming it: the pick is over.
        if (isFinishing && !completed) { completed = true; AttachmentPickRegistry.complete(intent.getStringExtra("request"), null, null); cameraFile?.delete() }
        work.cancel(); super.onDestroy()
    }
}
