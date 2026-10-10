package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

fun collabId(name: String): String = "01999999-0000-7000-8000-" + (listOf("home", "self", "item", "list", "comment", "attachment", "a", "b", "c", "one", "two", "occurrence", "deleted").indexOf(name) + 1).toString().padStart(12, '0')
private val collabNow = Instant.parse("2026-10-06T12:00:00Z")
fun listFixture(occurrence: String? = null) = ListResponse(collabId("list"), collabId("home"), collabId("self"), "Groceries", if (occurrence == null) ListMode.persistent else ListMode.per_occurrence,
    ListVisibility.household, 1, collabNow, collabNow, emptyList(), listOf("view", "manage", "check"),
    listOf(entryFixture("a", "a0"), entryFixture("b", "a1")), emptyList(), itemId = if (occurrence == null) null else collabId("item"), occurrenceId = occurrence?.let(::collabId))
fun entryFixture(id: String, position: String) = EntryResponse(collabId(id), collabId("home"), collabId("list"), id, "", "", position, 1, collabNow, collabNow)
fun commentFixture() = Comment(collabId("comment"), collabId("item"), "Hello @Alex", listOf(collabId("self")), collabNow, 1, authorId = collabId("self"))
fun attachmentFixture() = Attachment(collabId("attachment"), collabId("item"), "notes.txt", collabNow, 1, 5, "text/plain", false, uploaderId = collabId("self"))

class CollaborationTest {
    @Test fun atomicSnapshotAndEvictionCoverCollaboration() = runTest {
        val cache = memoryCache { collabNow }
        try {
            val snapshot = emptySnapshot(4).copy(lists = listOf(listFixture().copy(itemId = collabId("item"))), comments = listOf(commentFixture()), attachments = listOf(attachmentFixture()))
            cache.install(snapshot)
            assertEquals(snapshot.lists, cache.view.value.lists)
            assertEquals(snapshot.comments, cache.view.value.comments)
            assertEquals(snapshot.attachments, cache.view.value.attachments)
            assertFailsWith<IllegalStateException> { cache.installStaged(emptySnapshot(5), beforeCommit = { error("rollback") }) }
            assertEquals(4, cache.sync.value?.cursor)
            assertEquals(1, cache.view.value.lists.size)
            cache.evictItem(collabId("item"))
            assertTrue(cache.view.value.lists.isEmpty()); assertTrue(cache.view.value.comments.isEmpty()); assertTrue(cache.view.value.attachments.isEmpty())
            cache.putList(listFixture()); cache.clear()
            assertTrue(cache.view.value.lists.isEmpty())
        } finally { cache.close() }
    }
    @Test fun scopeNeverReusesAnotherOccurrencesChecks() = runTest {
        val cache = memoryCache { collabNow }
        try {
            cache.putList(listFixture("one")); cache.putList(listFixture("two"))
            assertEquals(setOf(collabId("one"), collabId("two")), cache.view.value.lists.map { it.occurrenceId }.toSet())
        } finally { cache.close() }
    }
    @Test fun commentDeadlineAndMentionHighlightUseServerAuthority() {
        val c = commentFixture()
        assertTrue(canEditComment(c, collabId("self"), collabNow + 15.minutes))
        assertFalse(canEditComment(c, collabId("self"), collabNow + 15.minutes + kotlin.time.Duration.parse("1ns")))
        assertFalse(canEditComment(c, "other", collabNow))
        val members = listOf(MemberRead(false,collabId("self"),collabId("home"),"Alex","user","#FABD2F","A","UTC","manual",1))
        assertEquals(listOf(6..10), mentionRanges(c, members))
        assertTrue(mentionRanges(c.copy(mentions = emptyList()), members).isEmpty())
    }
    @Test fun uploadRejectsSizeBeforeTransport() = runTest {
        var sent = false
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject { sent = true; error("not sent") }
        }
        val cache = memoryCache { collabNow }
        try {
            val repo = AttachmentRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            assertFailsWith<AttachmentTooLarge> { repo.upload(collabId("item"), null, "large.txt", "text/plain", ByteArray(MAX_ATTACHMENT_BYTES + 1)) }
            assertFalse(sent)
        } finally { cache.close() }
    }
    @Test fun gotItAndOccurrenceBodyAreVersioned() = runTest {
        val calls = mutableListOf<String>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            if (req.method == HttpMethod.Get) return@MockEngine respond(WireJson.encodeToString(ListResponse.serializer(), listFixture("occurrence")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            calls += req.body.toByteArray().decodeToString()
            assertEquals("7", req.headers["If-Match"])
            assertEquals("same", req.headers["Idempotency-Key"])
            respond(WireJson.encodeToString(EntryResponse.serializer(), entryFixture("a", "a0").copy(occurrenceId = collabId("occurrence"), checkedAt = collabNow, checkedBy = collabId("self"))), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        val cache = memoryCache { collabNow }
        try {
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            assertIs<WriteResult.Success<EntryResponse>>(repo.check(collabId("a"), 7, true, collabId("occurrence"), key = "same"))
            assertEquals(collabId("occurrence"), WireJson.parseToJsonElement(calls.first()).jsonObject["occurrence_id"]?.jsonPrimitive?.content)
        } finally { cache.close(); transport.close() }
    }
    @Test fun staleMoveRefetchesVisibleNeighboursAndUsesNewKey() = runTest {
        val requests = mutableListOf<Pair<String?, JsonObject>>()
        val current = listFixture().copy(propertyEntries = listOf(entryFixture("a", "a0"), entryFixture("c", "a1"), entryFixture("b", "a2")))
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            if (req.method == HttpMethod.Get) return@MockEngine respond(WireJson.encodeToString(ListResponse.serializer(), current), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            requests += req.headers["Idempotency-Key"] to WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
            if (requests.size == 1) respond("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Invalid move","errors":[{"field":"after_id","code":"stale-neighbour"}]}""", HttpStatusCode.UnprocessableEntity, headersOf(HttpHeaders.ContentType, "application/problem+json"))
            else respond(WireJson.encodeToString(EntryResponse.serializer(), entryFixture("a", "a3")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        val cache = memoryCache { collabNow }
        try {
            val replay = ListMoveReplay()
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator(newKey = { "replacement" }))
            assertIs<WriteResult.Success<EntryResponse>>(repo.move(listFixture(), entryFixture("a", "a0"), 1, replay, "original"))
            assertEquals(listOf("original", "replacement"), requests.map { it.first })
            assertEquals(collabId("c"), requests.last().second["after_id"]?.jsonPrimitive?.content)
            assertEquals(collabId("b"), requests.last().second["before_id"]?.jsonPrimitive?.content)
            assertEquals("replacement", replay.activeKey)
        } finally { cache.close(); transport.close() }
    }
    @Test fun uploadBusyRetryUsesSameKeyAndDownloadUsesPrivateCache() = runTest {
        val keys = mutableListOf<String?>(); var downloads = 0
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            assertEquals("1.0.0", req.headers["Porchlight-Client-Version"])
            if (req.method == HttpMethod.Get) { downloads++; respond("hello", headers = headersOf(HttpHeaders.ContentType, "text/plain")) }
            else {
                keys += req.headers["Idempotency-Key"]
                if (keys.size == 1) respond("""{"type":"urn:porchlight:problem:conflict","status":409,"title":"Busy"}""", HttpStatusCode.Conflict, headersOf(HttpHeaders.ContentType to listOf("application/problem+json"), HttpHeaders.RetryAfter to listOf("1")))
                else respond(WireJson.encodeToString(Attachment.serializer(), attachmentFixture()), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        val cache = memoryCache { collabNow }
        try {
            val repo = AttachmentRepository(GeneratedCall({ transport }, { "session" }), cache, WriteCoordinator(wait = {}), now = { collabNow })
            assertIs<WriteResult.Success<Attachment>>(repo.upload(collabId("item"), null, "notes.txt", "text/plain", "hello".encodeToByteArray(), "upload-key"))
            assertEquals(listOf<String?>("upload-key", "upload-key"), keys)
            repeat(2) { assertEquals("hello", repo.download(attachmentFixture()).decodeToString()) }
            assertEquals(1, downloads)
            cache.evictItem(collabId("item"))
            assertFailsWith<IllegalArgumentException> { repo.download(attachmentFixture()) }
        } finally { cache.close(); transport.close() }
    }

    @Test fun newerCollaborationResponsesSurviveOlderSnapshotWithoutResurrectingMissingRows() = runTest {
        val cache = memoryCache { collabNow }
        try {
            val old = emptySnapshot(2).copy(items = listOf(itemRead(collabId("item"))), lists = listOf(listFixture()), comments = listOf(commentFixture()), attachments = listOf(attachmentFixture()))
            cache.install(old)
            val mark = cache.receiptMark()
            cache.putList(listFixture().copy(version = 3, name = "Updated groceries"))
            cache.putComments(listOf(commentFixture().copy(version = 2, body = "New body")))
            cache.putAttachment(attachmentFixture().copy(version = 2))
            cache.installStaged(old, receivedAfter = mark)
            assertEquals("Updated groceries", cache.view.value.lists.single().name)
            assertEquals("New body", cache.view.value.comments.single().body)
            assertEquals(2, cache.view.value.attachments.single().version)
            val mark2 = cache.receiptMark()
            cache.putComments(listOf(commentFixture().copy(version = 3)))
            cache.installStaged(emptySnapshot(3), receivedAfter = mark2)
            assertTrue(cache.view.value.comments.isEmpty())
        } finally { cache.close() }
    }

    @Test fun sealedEntryReplyUpdatesGotItWithoutGuessingParentVersion() = runTest {
        val cache = memoryCache { collabNow }
        try {
            cache.putList(listFixture())
            cache.putEntry(entryFixture("a", "a0").copy(version = 2, checkedBy = collabId("self"), checkedAt = collabNow))
            assertEquals(listOf("a"), cache.view.value.lists.single().gotIt.map { it.label })
            assertEquals(1, cache.view.value.lists.single().version)
            assertEquals(listOf("b"), cache.view.value.lists.single().propertyEntries.map { it.label })
        } finally { cache.close() }
    }

    @Test fun eachAuthorsCommentHasItsOwnEditDeadline() {
        val first = commentFixture()
        val second = first.copy(id = collabId("deleted"), createdAt = collabNow + 5.minutes)
        assertEquals(collabNow + 15.minutes, nextCommentEditDeadline(listOf(first, second), collabId("self"), collabNow))
        assertEquals(collabNow + 20.minutes, nextCommentEditDeadline(listOf(first, second), collabId("self"), collabNow + 16.minutes))
        assertNull(nextCommentEditDeadline(listOf(first, second), collabId("self"), collabNow + 21.minutes))
    }

}
