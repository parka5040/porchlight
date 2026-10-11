package app.porchlight.shared.data

import app.porchlight.shared.api.models.CommentPage
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient3bHistoryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun partialTailDoesNotProveCoverageAndCursorAndWindowSurviveReopen() = runTest {
        val root = File(System.getProperty("porchlight.test.dir")).also { it.mkdirs() }
        val directory = Files.createTempDirectory(root.toPath(), "thread-history-").toFile()
        val file = File(directory, "read-cache.db")
        val row = commentFixture()
        var cache = desktopReadCache(file) { now }
        try {
            cache.putComments(listOf(row)) // a live tail, with unknown older history
            cache.inSession { fence -> cache.putCommentPage(fence, row.itemId, "unseen-cursor", CommentPage(emptyList(), null)) }
            assertNull(cache.view.value.commentHistory[row.itemId], "An out-of-order final page must not close an unknown gap")
            cache.inSession { fence ->
                cache.putCommentPage(fence, row.itemId, null, CommentPage(listOf(row.copy(id = "oldest")), "next-page"))
                cache.setCommentWindow(fence, row.itemId, 2)
            }
            assertFalse(cache.view.value.commentHistory.getValue(row.itemId).complete)
            cache.close()
            cache = desktopReadCache(file) { now }
            cache.initialize()
            assertEquals(CommentHistory("next-page", true, false, 2), cache.view.value.commentHistory[row.itemId])
            cache.inSession { fence -> cache.putCommentPage(fence, row.itemId, "next-page", CommentPage(listOf(row), null)) }
            assertTrue(cache.view.value.commentHistory.getValue(row.itemId).complete)
            cache.evictItem(row.itemId)
            assertTrue(cache.view.value.commentHistory.isEmpty())
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead(row.itemId))))
            assertEquals(CommentHistory(started = true, complete = true), cache.view.value.commentHistory[row.itemId])
            cache.clear()
            assertTrue(cache.view.value.commentHistory.isEmpty(), "Another identity must not inherit history coverage/window")
        } finally { cache.close(); directory.deleteRecursively() }
    }

    @Test fun lateEmptyPageCannotReinstallHistoryAfterVisibilityEviction() = runTest {
        val cache = memoryCache { now }
        val itemId = commentFixture().itemId
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(itemId))))
            cache.inSession { fence ->
                cache.evictItem(itemId)
                assertFailsWith<StaleVisibilityReply> { cache.putCommentPage(fence, itemId, null, CommentPage(emptyList(), null)) }
            }
            assertTrue(cache.view.value.commentHistory.isEmpty())
        } finally { cache.close() }
    }
}
