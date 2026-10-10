package app.porchlight.shared.screens.collaboration

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.lists.ListContent
import app.porchlight.shared.ui.*
import kotlinx.coroutines.*
import kotlin.time.Instant

// DESIGN-PENDING: collaboration forms, picker/upload, compare and recovery layouts.
@Composable fun CollaborationSection(row: ItemRead, navigate: (AppRoute) -> Unit) {
    val services = LocalScreenServices.current ?: return
    val data = services.data ?: return
    val view by data.cache.view.collectAsState()
    val writer = screenWrites("collaboration:${row.id}")
    val scope = applicationWriteScope()
    val readScope = rememberCoroutineScope()
    var text by remember(row.id) { mutableStateOf("") }
    var edit by remember(row.id) { mutableStateOf<Comment?>(null) }
    var error by remember(row.id) { mutableStateOf<String?>(null) }
    var upload by remember(row.id) { mutableStateOf<Job?>(null) }
    var progress by remember(row.id) { mutableStateOf<UploadProgress?>(null) }
    var attachedIds by remember(row.id) { mutableStateOf<Set<String>>(emptySet()) }
    var newList by remember(row.id) { mutableStateOf("") }
    var olderPages by remember(row.id) { mutableIntStateOf(0) }
    var nextCursor by remember(row.id) { mutableStateOf<String?>(null) }
    var historyExhausted by remember(row.id) { mutableStateOf(false) }
    var pageLoading by remember(row.id) { mutableStateOf(false) }
    var pageError by remember(row.id) { mutableStateOf<String?>(null) }
    var attachmentOffset by remember(row.id) { mutableIntStateOf(0) }
    var previewId by remember(row.id) { mutableStateOf<String?>(null) }
    var openingAttachment by remember(row.id) { mutableStateOf(false) }
    val comments = visibleThreadComments(view.comments, row.id, olderPages)
    val cachedCount = view.comments.count { it.itemId == row.id }
    val commentIds = comments.mapTo(mutableSetOf()) { it.id }
    val attachments = visibleThreadAttachments(view.attachments, row.id, commentIds, attachmentOffset)
    suspend fun showOlder() {
        if (pageLoading) return
        pageLoading = true
        try {
            // Snapshot metadata is complete in normal use. The API is oldest-first, not a tail
            // endpoint: fetch a single history page only when explicitly requested and needed.
            if (cachedCount <= comments.size && !historyExhausted) {
                val page = data.comments.page(row.id, nextCursor)
                nextCursor = page.nextCursor; historyExhausted = page.nextCursor == null
            }
            olderPages++
            attachmentOffset = 0; previewId = null; pageError = null
        } finally { pageLoading = false }
    }

    val memberId = view.settings?.memberId
    val admin = view.members.find { it.id == memberId }?.role == "super_admin"
    var now by remember(row.id) { mutableStateOf(services.clock.now()) }
    // Wake exactly at the nearest server edit deadline (clock injected); no stale edit button after 15 minutes.
    LaunchedEffect(comments, services.clock, memberId, row.id) {
        while (true) {
            now = services.clock.now()
            val deadline = nextCommentEditDeadline(comments, memberId, now) ?: break
            delay((deadline - now).inWholeMilliseconds.coerceAtLeast(0) + 1)
        }
    }
    val startUpload: (PickedAttachment, String?) -> Unit = { picked, target ->
        val c = view.comments.find { it.id == target }
        if ("comment" !in row.capabilities || (target != null && (c == null || !canEditComment(c, memberId, services.clock.now())))) error = "Attachment is no longer allowed. Refetch."
        else {
            upload = scope.launch { writer.submit { key -> data.attachments.upload(row.id, target, picked.name, picked.mime, picked.bytes, key) { sent, total -> progress = UploadProgress(sent, total) }.feedback() } }
        }
    }
    val latestUpload by rememberUpdatedState(startUpload)
    val picks = services.screenSession?.attachmentPicks ?: remember { AttachmentPicks() }
    val actions = rememberAttachmentActions(picks, row.id) { error = it }
    // C5-06: picker/camera results are parked in the session (they may return after a rotation or PIN relock
    // discarded this composition) and consumed only here, i.e. only while this item is shown and unlocked.
    LaunchedEffect(picks, row.id) {
        snapshotFlow { picks.waiting(row.id) }.collect { outcomes ->
            outcomes.forEach { outcome -> if (picks.take(outcome)) {
                outcome.error?.let { error = it }
                outcome.picked?.let { latestUpload(it, outcome.request.commentId) }
            } }
        }
    }
    val refresh: () -> Unit = { readScope.launch { try { data.comments.refetchByItemIds(setOf(row.id)); attachedIds = data.lists.refresh(row.id).mapTo(mutableSetOf()) { it.id }; data.sync.refresh(); pageError = null } catch (e: CancellationException) { throw e } catch (_: Exception) { pageError = "Collaboration could not refresh; cached content may be stale." } } }
    LaunchedEffect(row.id) { try { attachedIds = data.lists.refresh(row.id).mapTo(mutableSetOf()) { it.id } } catch (e: CancellationException) { throw e } catch (_: Exception) { pageError = "Could not refresh collaboration." } }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        ShellButton("Refresh collaboration", onClick = refresh)
        error?.let { Text(it) }
        pageError?.let { Text(it) }
        WriteStatus(writer, onDiscard = refresh)
        val state by writer.state.collectAsState()
        if (state.conflict) { Text("Your draft: $text"); Text("Current server value: ${state.current}"); Text("Refetch, then explicitly edit the current resource. Nothing is automatically reapplied.") }
        Text("Lists", style = MaterialTheme.typography.titleLarge)
        view.lists.filter { it.itemId == row.id || it.id in attachedIds }.distinctBy { it.id }.forEach { list ->
            ShellButton("Open ${list.name}", onClick = { navigate(AppRoute.Lists(list.id, if (list.mode == ListMode.per_occurrence) row.occurrence?.id else null)) })
        }
        if ("edit" in row.capabilities) {
            OutlinedTextField(newList, { newList = it }, label = { Text("New attached list") }, modifier = Modifier.fillMaxWidth())
            ShellButton("Create persistent attached list", enabled = newList.isNotBlank(), onClick = { val body = CreateList(newList.trim(), itemId = row.id, mode = ListMode.persistent); scope.launch { writer.submit { data.lists.create(body, it).also { result -> if (result is WriteResult.Success && newList.trim() == body.name) newList = "" }.feedback() } } })
            if (row.rrule != null) ShellButton("Create per-occurrence list", enabled = newList.isNotBlank(), onClick = { val body = CreateList(newList.trim(), itemId = row.id, mode = ListMode.per_occurrence); scope.launch { writer.submit { data.lists.create(body, it).also { result -> if (result is WriteResult.Success && newList.trim() == body.name) newList = "" }.feedback() } } })
        }
        Text("Comments", style = MaterialTheme.typography.titleLarge)
        if (pageLoading) Text("Loading comment page…")
        ShellButton("Show latest", enabled = !pageLoading && olderPages > 0, onClick = { olderPages = 0; attachmentOffset = 0; previewId = null })
        ShellButton("Show older", enabled = !pageLoading && (cachedCount > comments.size || !historyExhausted), onClick = { readScope.launch { try { showOlder() } catch (e: CancellationException) { throw e } catch (_: Exception) { pageError = "Could not load older comments; cached content is still available." } } })

        comments.forEach { comment ->
            CommentRow(comment, view.members, memberId, admin, now, { edit = comment; text = comment.body }, { scope.launch { writer.submit { data.comments.delete(comment, it).feedback() } } })
            if (canEditComment(comment, memberId, now) && "comment" in row.capabilities) ShellButton("Attach to comment by ${view.members.find { it.id == comment.authorId }?.displayName ?: "Former member or agent"}", onClick = { actions.file(comment.id) })
        }
        if ("comment" in row.capabilities) {
            OutlinedTextField(text, { text = it }, label = { Text(if (edit == null) "Add a comment" else "Edit comment") }, modifier = Modifier.fillMaxWidth())
            Row(Modifier.horizontalScroll(rememberScrollState())) { view.members.forEach { m -> ShellButton("Mention ${m.displayName}", onClick = { text += " @${m.displayName}" }) } }
            ShellButton(if (edit == null) "Post comment" else "Save comment", enabled = text.isNotBlank() && (edit == null || canEditComment(edit!!, memberId, now)), onClick = {
                val body = text; val current = edit
                scope.launch { writer.submit { key -> (if (current == null) data.comments.create(row.id, body, key) else data.comments.edit(current, body, key)).also { result ->
                    if (result is WriteResult.Success && edit == current) {
                        if (text == body) { text = ""; edit = null }
                        else if (current != null) edit = result.value // retain newer typing with the committed version
                    }
                }.feedback() } }
            })
            if (edit != null) ShellButton("Cancel comment edit", onClick = { edit = null; text = "" })
        }
        Text("Attachments", style = MaterialTheme.typography.titleLarge)
        attachments.forEach { attachment ->
            key(attachment.id, attachment.version) {
            val comment = attachment.commentId?.let { id -> view.comments.find { it.id == id } }
            val deletable = if (attachment.commentId != null) admin || (comment != null && canEditComment(comment, memberId, now)) else attachment.uploaderId == memberId || "edit" in row.capabilities
            AttachmentRow(attachment, deletable, { if (!openingAttachment) { openingAttachment = true; readScope.launch {
                try { data.attachments.open(attachment) { bytes -> actions.open(attachment.id, attachment.contentType, bytes) } }
                catch (e: CancellationException) { throw e } catch (e: Exception) { error = attachmentOpenError(e) }
                finally { openingAttachment = false }
            } } }, { scope.launch { writer.submit { data.attachments.delete(attachment, it).feedback() } } })
            if (attachment.hasThumbnail) {
                ShellButton(if (previewId == attachment.id) "Hide thumbnail" else "Show thumbnail", onClick = { previewId = if (previewId == attachment.id) null else attachment.id })
                if (previewId == attachment.id) {
                var thumbnail by remember(attachment.id, attachment.version) { mutableStateOf<ByteArray?>(null) }
                LaunchedEffect(attachment.id, attachment.version) { try { thumbnail = data.attachments.download(attachment, true) } catch (e: CancellationException) { throw e } catch (_: Exception) { /* row still opens the original */ } }
                thumbnail?.let { AttachmentThumbnail(it, "Thumbnail ${attachment.originalName}", Modifier.sizeIn(maxWidth = 240.dp, maxHeight = 180.dp)) }
                }
            }
            }
        }
        if (openingAttachment) Text("Opening attachment…")
        ShellButton("Previous attachments", enabled = attachmentOffset > 0, onClick = { attachmentOffset = (attachmentOffset - ATTACHMENT_WINDOW_SIZE).coerceAtLeast(0); previewId = null })
        ShellButton("Next attachments", enabled = visibleThreadAttachments(view.attachments, row.id, commentIds, attachmentOffset + ATTACHMENT_WINDOW_SIZE).isNotEmpty(), onClick = { attachmentOffset += ATTACHMENT_WINDOW_SIZE; previewId = null })
        if ("comment" in row.capabilities) {
            ShellButton("Pick photo", onClick = { actions.photo() })
            if (services.platform.kind == app.porchlight.shared.PlatformKind.Android) ShellButton("Capture photo", onClick = { actions.camera() })
            ShellButton("Choose file", onClick = { actions.file() })
        }
        if (upload?.isActive == true) {
            Text("Uploading ${progress?.sent ?: 0} / ${progress?.total ?: 0} bytes")
            ShellButton("Cancel upload", onClick = { upload?.cancel() })
        }
    }
}
@Composable fun CommentRow(comment: Comment, members: List<MemberRead>, memberId: String?, admin: Boolean, now: Instant, edit: () -> Unit, delete: () -> Unit) {
    PorchlightCard(Modifier.fillMaxWidth()) {
        if (comment.deletedAt != null) Text("Comment deleted") else {
            Text(members.find { it.id == comment.authorId }?.displayName ?: "Former member or agent")
            val color = MaterialTheme.colorScheme.primary
            Text(buildAnnotatedString { append(comment.body); mentionRanges(comment, members).forEach { addStyle(SpanStyle(color = color, fontWeight = androidx.compose.ui.text.font.FontWeight.Bold), it.first, it.last + 1) } })
            Text(comment.createdAt.toString() + if (comment.editedAt != null) " · edited" else "")
            if (canEditComment(comment, memberId, now)) ShellButton("Edit comment", onClick = edit)
            if (admin || (memberId != null && comment.authorId == memberId)) ShellButton("Delete comment", onClick = delete)
        }
    }
}
@Composable fun AttachmentRow(row: Attachment, canDelete: Boolean, open: () -> Unit, delete: () -> Unit) {
    PorchlightCard(Modifier.fillMaxWidth()) {
        Text(row.originalName, style = MaterialTheme.typography.titleMedium)
        Text("${row.contentType} · ${row.propertySize} bytes")
        ShellButton("Open ${row.originalName}", onClick = open)
        if (canDelete) ShellButton("Delete attachment ${row.originalName}", onClick = delete)
    }
}
