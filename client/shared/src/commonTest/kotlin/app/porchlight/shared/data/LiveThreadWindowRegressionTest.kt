package app.porchlight.shared.data

import app.porchlight.shared.api.models.Comment
import app.porchlight.shared.screens.collaboration.visibleThreadComments
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.seconds

class LiveThreadWindowRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun history(size: Int): List<Comment> = List(size) {
        commentFixture().copy(id = "comment-${it.toString().padStart(3, '0')}", createdAt = now + it.seconds)
    }

    @Test fun opensOnNewestFiftyEvenWithUnorderedOfflineCache() {
        val comments = history(120)
        assertEquals(comments.takeLast(50), visibleThreadComments(comments, collabId("item")))
        assertEquals(comments.takeLast(50), visibleThreadComments(comments.shuffled(kotlin.random.Random(7)), collabId("item")))
    }

    @Test fun showOlderAddsPreviousCachedPageAndKeepsTheLiveTail() {
        val comments = history(120)
        assertEquals(comments.takeLast(100), visibleThreadComments(comments, collabId("item"), olderPages = 1))
        assertEquals(comments, visibleThreadComments(comments, collabId("item"), olderPages = 2))
        assertEquals(history(121).takeLast(100), visibleThreadComments(history(121), collabId("item"), olderPages = 1))
    }

    @Test fun cacheInsertionAppearsInAlreadyOpenWindow() = runTest {
        val cache = memoryCache { now }
        try {
            val comments = history(3)
            cache.putComments(comments)
            assertEquals(comments, visibleThreadComments(cache.view.value.comments, collabId("item")))
            val synced = history(4).last()
            cache.putComments(listOf(synced))
            assertEquals(comments + synced, visibleThreadComments(cache.view.value.comments, collabId("item")))
        } finally { cache.close() }
    }

    @Test fun ownPostOnFullWindowAppearsWithoutRefresh() = runTest {
        val cache = memoryCache { now }
        val comments = history(120)
        val posted = history(121).last()
        val transport = KtorApiTransport(HttpClient(MockEngine {
            respond(WireJson.encodeToString(posted), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            cache.putComments(comments)
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            // Isolate the committed local insertion: no snapshot/manual refresh is needed.
            data.comments.afterWrite = {}
            assertIs<WriteResult.Success<Comment>>(data.comments.create(collabId("item"), "new", "post-key"))
            val visible = visibleThreadComments(cache.view.value.comments, collabId("item"))
            assertEquals((comments + posted).takeLast(50), visible)
        } finally { transport.close(); cache.close() }
    }
}
