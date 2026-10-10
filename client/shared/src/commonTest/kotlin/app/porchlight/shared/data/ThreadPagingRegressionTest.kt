package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class ThreadPagingRegressionTest {
    @Test fun openingThreadRequestsOnePageNotEntireHistory() = runTest {
        val cache = memoryCache { Instant.parse("2026-10-06T12:00:00Z") }
        val requests = mutableListOf<String?>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            requests += req.url.parameters["cursor"]
            val index = req.url.parameters["cursor"]?.toIntOrNull() ?: 0
            val rows = List(50) { commentFixture().copy(id = "comment-${index * 50 + it}") }
            respond(WireJson.encodeToString(CommentPage(rows, if (index < 99) "${index + 1}" else null)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { Instant.parse("2026-10-06T12:00:00Z") })
            val rows = data.comments.refresh(collabId("item"))
            assertEquals(1, requests.size, "Opening must not walk 100 pages")
            assertEquals(50, rows.size)
            val next = data.comments.page(collabId("item"), "1")
            assertEquals(2, requests.size)
            assertEquals("1", requests.last())
            assertEquals("2", next.nextCursor)
            assertEquals("comment-50", next.items.first().id)
            // A partial page must not evict off-page metadata/attachments as if they were deleted.
            assertEquals(100, cache.view.value.comments.size)
        } finally { transport.close(); cache.close() }
    }
    @Test fun cachedHistoryDoesNotComposeAllCommentsOrCommentThumbnails() {
        val comments = List(5000) { commentFixture().copy(id = "comment-$it") }
        val attachments = List(5000) { attachmentFixture().copy(id = "attachment-$it", commentId = "comment-$it", hasThumbnail = true) }
        val visible = visibleThreadComments(comments, collabId("item"))
        assertEquals(COMMENT_WINDOW_SIZE, visible.size)
        val ids = visible.mapTo(mutableSetOf()) { it.id }
        val first = visibleThreadAttachments(attachments, collabId("item"), ids)
        assertEquals(ATTACHMENT_WINDOW_SIZE, first.size)
        assertTrue(first.all { it.commentId in ids })
        val next = visibleThreadAttachments(attachments, collabId("item"), ids, ATTACHMENT_WINDOW_SIZE)
        assertEquals(ATTACHMENT_WINDOW_SIZE, next.size)
        assertTrue(first.none { a -> next.any { it.id == a.id } })
        // Older metadata is composed only after an explicit page request; the live tail remains.
        assertEquals(COMMENT_WINDOW_SIZE * 2, visibleThreadComments(comments, collabId("item"), olderPages = 1).size)
    }
}
