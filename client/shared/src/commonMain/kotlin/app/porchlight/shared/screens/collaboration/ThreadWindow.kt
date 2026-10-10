package app.porchlight.shared.screens.collaboration

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.COMMENT_PAGE_LIMIT

const val COMMENT_WINDOW_SIZE = COMMENT_PAGE_LIMIT
const val ATTACHMENT_WINDOW_SIZE = 20

/** Live cache tail. Only explicit Show older actions expand the metadata presentation budget. */
fun visibleThreadComments(rows: List<Comment>, itemId: String, olderPages: Int = 0): List<Comment> {
    val budget = ((olderPages.coerceAtLeast(0).toLong() + 1) * COMMENT_WINDOW_SIZE).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
    val order = compareBy<Comment> { it.createdAt }.thenBy { it.id }
    val newest = mutableListOf<Comment>()
    // Keep only the presentation budget even while selecting from a complete cached history.
    rows.forEach { row ->
        if (row.itemId == itemId && (newest.size < budget || order.compare(row, newest.first()) > 0)) {
            val position = newest.binarySearch(row, order).let { if (it < 0) -it - 1 else it }
            newest.add(position, row)
            if (newest.size > budget) newest.removeAt(0)
        }
    }
    return newest
}

/** Only displayed comment attachments and item attachments are eligible, in bounded batches. */
fun visibleThreadAttachments(rows: List<Attachment>, itemId: String, commentIds: Set<String>, offset: Int = 0): List<Attachment> =
    rows.asSequence().filter { it.itemId == itemId && it.deletedAt == null && (it.commentId == null || it.commentId in commentIds) }
        .drop(offset.coerceAtLeast(0)).take(ATTACHMENT_WINDOW_SIZE).toList()
