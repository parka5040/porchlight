package app.porchlight.shared.screens.collaboration

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier

data class PickedAttachment(val name: String, val mime: String, val bytes: ByteArray)
/** Pickers target the item, or the comment [commentId] chosen at launch; results arrive through [AttachmentPicks]. */
interface AttachmentActions {
    fun photo(commentId: String? = null)
    fun file(commentId: String? = null)
    fun camera(commentId: String? = null)
    suspend fun open(id: String, mime: String, bytes: ByteArray)
}
/** Results go to [picks] for [itemId], never to a composition-bound callback; [error] reports launch failures only. */
@Composable expect fun rememberAttachmentActions(picks: AttachmentPicks, itemId: String, error: (String) -> Unit): AttachmentActions
@Composable expect fun AttachmentThumbnail(bytes: ByteArray, description: String, modifier: Modifier = Modifier)

/** Whitelisted viewer suffix shared by both platforms; never uses the server filename. */
fun attachmentExtension(mime: String): String = when (mime) {
    "application/pdf" -> "pdf"; "text/plain" -> "txt"; "image/png" -> "png"
    "image/webp" -> "webp"; "image/gif" -> "gif"; "image/jpeg" -> "jpg"
    else -> "bin"
}
fun attachmentMimeFromName(name: String): String? = when (name.substringAfterLast('.', "").lowercase()) {
    "pdf" -> "application/pdf"; "txt" -> "text/plain"; "png" -> "image/png"
    "webp" -> "image/webp"; "gif" -> "image/gif"; "jpg", "jpeg" -> "image/jpeg"
    else -> null
}

/**
 * User-facing text for a failed attachment open. A private-cache directory problem is persistent, so it gets its own
 * message naming the folder and the fix; download, visibility and lock failures keep the generic retry message.
 */
fun attachmentOpenError(failure: Exception): String = when (failure) {
    is app.porchlight.shared.files.PrivateDirectoryUnavailable ->
        "Attachments can't open: Porchlight's private folder ${failure.path} is unusable because ${failure.reason}. " +
            "It must be a folder you own that only you can read. Fix or remove that folder, then open the attachment again; retrying alone won't help."
    else -> "Attachment could not open. Refetch or unlock before retrying."
}
