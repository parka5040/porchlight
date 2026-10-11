package app.porchlight.shared.screens.collaboration

import app.porchlight.shared.theme.PorchlightTokens

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

private class CommentEditorState {
    var text by mutableStateOf("")
    var edit by mutableStateOf<Comment?>(null)
}

// DESIGN-PENDING: collaboration forms, picker/upload, compare and recovery layouts.
@Composable fun CollaborationSection(row: ItemRead, navigate: (AppRoute) -> Unit) {
    val services = LocalScreenServices.current ?: return
    val data = services.data ?: return
    val view by data.cache.view.collectAsState()
    val writer = screenWrites("collaboration:${row.id}")
    val scope = applicationWriteScope()
    val readScope = rememberCoroutineScope()
    var retainedEditor by remember(row.id) { mutableStateOf<RetainedCacheValue<CommentEditorState>?>(null) }
    val editor = retainedEditor?.current(view)?.takeIf { draft ->
        draft.edit?.let { source -> view.comments.any { it.id == source.id && it.deletedAt == null } } != false
    } ?: CommentEditorState().also { fresh -> retainedEditor = view.retainDraft(setOf(CacheResource("Item", row.id)), fresh) }
    var text by editor::text
    var edit by editor::edit
    fun finishEdit() {
        text = ""; edit = null
        retainedEditor?.bindResources(data.cache.view.value, setOf(CacheResource("Item", row.id)))
    }
    var error by remember(row.id) { mutableStateOf<String?>(null) }
    val upload = services.screenSession?.upload(row.id) ?: remember(row.id) { UploadAttempt() }
    val progress by upload.progress.collectAsState()
    var attachedIds by remember(row.id) { mutableStateOf<Set<String>>(emptySet()) }
    var newList by remember(row.id) { mutableStateOf("") }
    val history = view.commentHistory[row.id] ?: CommentHistory()
    val olderPages = history.olderPages
    var pageLoading by remember(row.id) { mutableStateOf(false) }
    var pageError by remember(row.id) { mutableStateOf<String?>(null) }
    var attachmentOffset by remember(row.id) { mutableIntStateOf(0) }
    var previewId by remember(row.id) { mutableStateOf<String?>(null) }
    var openingAttachment by remember(row.id) { mutableStateOf(false) }
    val comments = remember(view.comments, row.id, olderPages) { visibleThreadComments(view.comments, row.id, olderPages) }
    val cachedCount = remember(view.comments, row.id) { view.comments.count { it.itemId == row.id } }
    val commentIds = remember(comments) { comments.mapTo(mutableSetOf()) { it.id } }
    val attachments = remember(view.attachments, row.id, commentIds, attachmentOffset) { visibleThreadAttachments(view.attachments, row.id, commentIds, attachmentOffset) }
    val hasMoreAttachments = remember(view.attachments, row.id, commentIds, attachmentOffset) {
        visibleThreadAttachments(view.attachments, row.id, commentIds, attachmentOffset + ATTACHMENT_WINDOW_SIZE).isNotEmpty()
    }
    val attachedLists = remember(view.lists, row.id, attachedIds) { view.lists.filter { it.itemId == row.id || it.id in attachedIds }.distinctBy { it.id } }
    suspend fun showOlder() {
        if (pageLoading) return
        pageLoading = true
        try {
            // Row count cannot prove coverage: an oldest-first page and a cached live tail may have a gap.
            // Load one page per explicit action until a snapshot or a contiguous walk proves completeness.
            if (!history.complete) data.comments.page(row.id, history.nextCursor)
            data.comments.window(row.id, (olderPages.toLong() + 1).coerceAtMost(Int.MAX_VALUE.toLong()).toInt())
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
            scope.launch { writer.submit { key -> upload.track { report ->
                data.attachments.upload(row.id, target, picked.name, picked.mime, picked.bytes, key, report).feedback()
            } } }
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
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        ShellButton("Refresh collaboration", onClick = refresh)
        error?.let { Text(it) }
        pageError?.let { Text(it) }
        WriteStatus(writer, onDiscard = refresh)
        val state by writer.state.collectAsState()
        if (state.conflict) { Text("Your draft: $text"); Text("Current server value: ${state.current}"); Text("Refetch, then explicitly edit the current resource. Nothing is automatically reapplied.") }
        Text("Lists", style = MaterialTheme.typography.titleLarge)
        attachedLists.forEach { list ->
            ShellButton("Open ${list.name}", onClick = { navigate(AppRoute.Lists(list.id, if (list.mode == ListMode.per_occurrence) row.occurrence?.id else null)) })
        }
        if ("edit" in row.capabilities) {
            OutlinedTextField(newList, { newList = it }, label = { Text("New attached list") }, modifier = Modifier.fillMaxWidth())
            ShellButton("Create persistent attached list", enabled = newList.isNotBlank(), onClick = { val body = CreateList(newList.trim(), itemId = row.id, mode = ListMode.persistent); scope.launch { writer.submit { data.lists.create(body, it).also { result -> if (result is WriteResult.Success && newList.trim() == body.name) newList = "" }.feedback() } } })
            if (row.rrule != null) ShellButton("Create per-occurrence list", enabled = newList.isNotBlank(), onClick = { val body = CreateList(newList.trim(), itemId = row.id, mode = ListMode.per_occurrence); scope.launch { writer.submit { data.lists.create(body, it).also { result -> if (result is WriteResult.Success && newList.trim() == body.name) newList = "" }.feedback() } } })
        }
        Text("Comments", style = MaterialTheme.typography.titleLarge)
        if (pageLoading) Text("Loading comment page…")
        if (!history.complete) Text("Cached comment history may have gaps. Show older loads the next history page.")
        ShellButton("Show latest", enabled = !pageLoading && olderPages > 0, onClick = { readScope.launch {
            try { data.comments.window(row.id, 0); attachmentOffset = 0; previewId = null }
            catch (e: CancellationException) { throw e } catch (_: Exception) { pageError = "Could not save the comment window." }
        } })
        ShellButton("Show older", enabled = !pageLoading && (cachedCount > comments.size || !history.complete), onClick = { readScope.launch { try { showOlder() } catch (e: CancellationException) { throw e } catch (_: Exception) { pageError = "Could not load older comments; cached content is still available." } } })

        comments.forEach { comment -> key(comment.id) {
            CommentRow(comment, view.members, memberId, admin, now, { edit = comment; text = comment.body; retainedEditor = view.retainDraft(setOf(CacheResource("Item", row.id), CacheResource("Comment", comment.id)), editor) }, { scope.launch { writer.submit { data.comments.delete(comment, it).feedback() } } })
            if (canEditComment(comment, memberId, now) && "comment" in row.capabilities) ShellButton("Attach to comment by ${view.members.find { it.id == comment.authorId }?.displayName ?: "Former member or agent"}", onClick = { actions.file(comment.id) })
        } }
        if ("comment" in row.capabilities) {
            OutlinedTextField(text, { text = it }, label = { Text(if (edit == null) "Add a comment" else "Edit comment") }, modifier = Modifier.fillMaxWidth())
            Row(Modifier.horizontalScroll(rememberScrollState())) { view.members.forEach { m -> ShellButton("Mention ${m.displayName}", onClick = { text += " @${m.displayName}" }) } }
            ShellButton(if (edit == null) "Post comment" else "Save comment", enabled = text.isNotBlank() && (edit == null || canEditComment(edit!!, memberId, now)), onClick = {
                val body = text; val current = edit
                scope.launch { writer.submit { key -> (if (current == null) data.comments.create(row.id, body, key) else data.comments.edit(current, body, key)).also { result ->
                    if (result is WriteResult.Success && retainedEditor?.current(data.cache.view.value) === editor && edit == current) {
                        if (text == body) finishEdit()
                        else {
                            // New typing belongs to the committed comment, not a second create containing it again.
                            edit = result.value
                            retainedEditor?.bindResources(data.cache.view.value, setOf(CacheResource("Item", row.id), CacheResource("Comment", result.value.id)))
                        }
                    }
                }.feedback() } }
            })
            if (edit != null) ShellButton("Cancel comment edit", onClick = ::finishEdit)
        }
        Text("Attachments", style = MaterialTheme.typography.titleLarge)
        attachments.forEach { attachment ->
            key(attachment.id, attachment.version) {
            val comment = attachment.commentId?.let { id -> view.comments.find { it.id == id } }
            val deletable = if (attachment.commentId != null) admin || (comment != null && canEditComment(comment, memberId, now)) else attachment.uploaderId == memberId || "edit" in row.capabilities
            AttachmentRow(attachment, deletable, { if (!openingAttachment) { openingAttachment = true; readScope.launch {
                try { data.attachments.openPrepared(attachment) { bytes -> actions.prepareViewer(attachment.id, attachment.contentType, bytes) } }
                catch (e: CancellationException) { throw e } catch (e: Exception) { error = attachmentOpenError(e) }
                finally { openingAttachment = false }
            } } }, { scope.launch { writer.submit { data.attachments.delete(attachment, it).feedback() } } })
            if (attachment.hasThumbnail) {
                ShellButton(if (previewId == attachment.id) "Hide thumbnail" else "Show thumbnail", onClick = { previewId = if (previewId == attachment.id) null else attachment.id })
                if (previewId == attachment.id) {
                var thumbnail by remember(attachment.id, attachment.version) { mutableStateOf<ByteArray?>(null) }
                LaunchedEffect(attachment.id, attachment.version) { try { thumbnail = data.attachments.download(attachment, true) } catch (e: CancellationException) { throw e } catch (_: Exception) { /* row still opens the original */ } }
                thumbnail?.let { AttachmentThumbnail(it, "Thumbnail ${attachment.originalName}", Modifier.sizeIn(maxWidth = PorchlightTokens.attachmentPreviewMaxWidth.dp, maxHeight = PorchlightTokens.attachmentPreviewMaxHeight.dp)) }
                }
            }
            }
        }
        if (openingAttachment) Text("Opening attachment…")
        ShellButton("Previous attachments", enabled = attachmentOffset > 0, onClick = { attachmentOffset = (attachmentOffset - ATTACHMENT_WINDOW_SIZE).coerceAtLeast(0); previewId = null })
        ShellButton("Next attachments", enabled = hasMoreAttachments, onClick = { attachmentOffset += ATTACHMENT_WINDOW_SIZE; previewId = null })
        if ("comment" in row.capabilities) {
            ShellButton("Pick photo", onClick = { actions.photo() })
            if (services.platform.kind == app.porchlight.shared.PlatformKind.Android) ShellButton("Capture photo", onClick = { actions.camera() })
            ShellButton("Choose file", onClick = { actions.file() })
        }
        if (progress != null) {
            Text("Uploading ${progress?.sent ?: 0} / ${progress?.total ?: 0} bytes")
            ShellButton("Cancel upload", onClick = upload::cancel)
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
