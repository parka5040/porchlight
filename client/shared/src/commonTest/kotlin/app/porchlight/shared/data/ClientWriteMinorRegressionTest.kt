package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class ClientWriteMinorRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })

    @Test fun split204KeepsHeadAndItsCollaborationWhenSnapshotFails() = runTest {
        val cache = memoryCache { now }
        var writes = 0
        val api = transport { req ->
            writes++
            assertEquals("this_and_future", req.url.parameters["scope"])
            respond("", HttpStatusCode.NoContent)
        }
        try {
            val id = collabId("item")
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(id)), comments = listOf(commentFixture()), attachments = listOf(attachmentFixture()), lists = listOf(listFixture())))
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            var refreshes = 0
            data.items.afterWrite = { assertEquals(id, it); refreshes++; error("Snapshot unavailable") }
            val result = assertIs<WriteResult.Success<ItemResponse?>>(data.items.edit(id, 1, ItemEdit().text("title", "tail"), EditScope.ThisAndFuture, collabId("occurrence"), "split"))
            assertNull(result.value); assertTrue(result.refreshPending)
            assertEquals(1, writes); assertEquals(1, refreshes)
            assertEquals(id, cache.view.value.items.single().id)
            assertEquals(commentFixture(), cache.view.value.comments.single())
            assertEquals(attachmentFixture(), cache.view.value.attachments.single())
            assertEquals(listFixture().id, cache.view.value.lists.single().id)
        } finally { api.close(); cache.close() }
    }

    @Test fun create204IsSavedAndReconcilesWithoutInventingAnId() = runTest {
        val cache = memoryCache { now }
        var creates = 0; var snapshots = 0
        val api = transport { req ->
            if (req.method == HttpMethod.Post) { creates++; respond("", HttpStatusCode.NoContent) }
            else { snapshots++; respond("", HttpStatusCode.ServiceUnavailable) }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            val body = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("one")))
            val screen = ScreenWrites { "create-204" }
            screen.submit { data.items.create(body, it).feedback() }
            assertTrue(screen.state.value.saved, screen.state.value.error)
            assertTrue(screen.state.value.refreshPending)
            assertNull(screen.state.value.retryKey)
            assertEquals(1, creates); assertEquals(1, snapshots)
            assertTrue(cache.view.value.items.isEmpty())
        } finally { api.close(); cache.close() }
    }

    @Test fun commentTickleReconcilesEditsAndDeletesBeyondFirstPage() = runTest {
        val cache = memoryCache { now }
        val comments = List(120) { commentFixture().copy(id = it.toString().padStart(3, '0')) }
        val edited = comments[80].copy(body = "edited on another device", version = 2)
        val server = comments.filter { it.id != comments[90].id }.map { if (it.id == edited.id) edited else it }
        val cursors = mutableListOf<String?>()
        val api = transport { req ->
            cursors += req.url.parameters["cursor"]
            val offset = req.url.parameters["cursor"]?.toInt() ?: 0
            assertEquals("50", req.url.parameters["limit"])
            val next = (offset + COMMENT_PAGE_LIMIT).takeIf { it < server.size }?.toString()
            respond(WireJson.encodeToString(CommentPage(server.drop(offset).take(COMMENT_PAGE_LIMIT), next)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            cache.putComments(comments)
            cache.putAttachment(attachmentFixture().copy(commentId = comments[90].id))
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.comments.refetchByItemIds(setOf(collabId("item")))
            assertEquals(listOf(null, "50", "100"), cursors)
            assertEquals(server.toSet(), cache.view.value.comments.toSet())
            assertEquals(edited, cache.view.value.comments.single { it.id == edited.id })
            assertTrue(cache.view.value.attachments.isEmpty())
        } finally { api.close(); cache.close() }
    }

    @Test fun commentTickleDoesNotWalkUncachedHistory() = runTest {
        val cache = memoryCache { now }
        var requests = 0
        val api = transport {
            requests++
            respond(WireJson.encodeToString(CommentPage(List(50) { commentFixture().copy(id = "$it") }, "next")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.comments.refetchByItemIds(setOf(collabId("item")))
            assertEquals(1, requests)
        } finally { api.close(); cache.close() }
    }

    @Test fun contentionWaitIsCappedPerAttemptAndPreservesKey() = runTest {
        for (type in listOf("idempotency-in-progress", "conflict")) {
            val waits = mutableListOf<Long>(); val keys = mutableListOf<String>()
            val coordinator = WriteCoordinator(wait = { waits += it })
            val failure = assertFailsWith<ApiProblem> {
                coordinator.execute("same-key") { key -> keys += key; throw ApiProblem(type, 409, "Busy", retryAfterMillis = 86_400_000L) }
            }
            assertEquals(type, failure.type)
            assertEquals(listOf(5000L, 5000L), waits)
            assertEquals(List(3) { "same-key" }, keys)
        }
    }
}
