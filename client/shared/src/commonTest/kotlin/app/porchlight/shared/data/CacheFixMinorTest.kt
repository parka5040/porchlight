package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlin.test.*
import kotlin.time.Instant

class CacheFixMinorTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun api(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun MockRequestHandleScope.notFound() = respond("""{"type":"urn:porchlight:problem:not-found","status":404,"title":"Unavailable"}""", HttpStatusCode.NotFound, headersOf(HttpHeaders.ContentType, "application/problem+json"))
    @Test fun agendaDateOverlapUsesStartThenDueThenLocalDay() {
        val date = LocalDate.parse("2026-10-06")
        val row = agenda("occ", due = date).agenda!!
        val late = now + kotlin.time.Duration.parse("24h")
        assertFalse(provablyInAgendaWindow(row.copy(startsAt = late), now, late))
        assertFalse(provablyInAgendaWindow(row.copy(dueAt = late), now, late))
        assertTrue(provablyInAgendaWindow(row, now, late))
    }
    @Test fun tzidLessFixedTodoKeepsOverrideLocalsWithoutInventingUtcInstants() {
        val row = occurrenceRead("occ", "one").copy(overrideLocalStart = "2026-10-06T23:30:00", overrideLocalEnd = "2026-10-07T00:30:00", overrideLocalDue = "2026-10-06T22:00:00")
        val effective = row.effective(itemRead("one").copy(timeMode = "fixed", tzid = null))
        assertNull(effective.startsAt); assertNull(effective.endsAt); assertNull(effective.dueAt)
        assertEquals(row.overrideLocalStart, effective.localStart)
        val draft = app.porchlight.shared.screens.item.scopeSeed(itemRead("one").copy(timeMode = "fixed", tzid = null, occurrence = row), app.porchlight.shared.screens.item.EditorScope.ThisOccurrence)
        assertEquals("", draft.startsAt); assertEquals(row.overrideLocalStart, draft.localStart)
    }
    @Test fun successfulItemReadClearsEarlierVisibilityDenialReceipt() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")))
            cache.install(snapshot); val mark = cache.receiptMark()
            cache.evictItem("one"); cache.putItem(itemResponse("one").copy(version = 2))
            cache.installStaged(snapshot.copy(cursor = 2), receivedAfter = mark)
            assertEquals(2, cache.view.value.items.single().version)
        } finally { cache.close() }
    }
    @Test fun memberCollectionReappliesOnlyWritesAfterItsRequest() = runTest {
        val cache = memoryCache { now }
        try {
            val member = MemberRead(false, "self", "household", "Self", "user", "#FABD2F", "S", "UTC", "manual", 1)
            cache.putMember(member.copy(version = 3)); val mark = cache.receiptMark()
            cache.putMember(member.copy(version = 4))
            cache.replaceMembers(listOf(member), receivedAfter = mark)
            assertEquals(4, cache.view.value.members.single().version)
            // A later collection request's own mark excludes the earlier receipt: authoritative lower version (R81).
            cache.replaceMembers(listOf(member), receivedAfter = cache.receiptMark())
            assertEquals(1, cache.view.value.members.single().version)
        } finally { cache.close() }
    }
    @Test fun terminalResnapshotIgnoresReceiptsEvenWithoutCursorRegression() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")))
            cache.install(snapshot); val mark = cache.receiptMark()
            cache.putItem(itemResponse("one").copy(version = 5))
            cache.installStaged(snapshot.copy(cursor = 2), receivedAfter = mark, authoritative = true)
            assertEquals(1, cache.view.value.items.single().version)
        } finally { cache.close() }
    }
    @Test fun lateEmptyOccurrenceWindowCannotDeleteANewerWriteButFreshFetchCan() = runTest {
        val cache = memoryCache { now }; val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val transport = api { started.complete(Unit); release.await(); respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json")) }
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one").copy(startsAt = now))))
            val job = async { data.occurrences.refresh("one", now, now + kotlin.time.Duration.parse("1h")) }
            started.await()
            cache.putOccurrence(occurrenceResponse("occ", "one").copy(version = 2, startsAt = now))
            release.complete(Unit); job.await()
            assertEquals(2, cache.view.value.occurrences.single().version)
            data.occurrences.refresh("one", now, now + kotlin.time.Duration.parse("1h"))
            assertTrue(cache.view.value.occurrences.isEmpty())
        } finally { release.complete(Unit); transport.close(); cache.close() }
    }
    @Test fun lateWindowOmissionCannotDeleteANewerSnapshotRow() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one").copy(startsAt = now)))
            cache.install(snapshot); val mark = cache.receiptMark()
            cache.install(snapshot.copy(cursor = 2, occurrences = listOf(snapshot.occurrences.single().copy(version = 2))))
            cache.putOccurrences("one", emptyList(), now, now + kotlin.time.Duration.parse("1h"), receivedAfter = mark)
            assertEquals(2, cache.view.value.occurrences.single().version)
        } finally { cache.close() }
    }
    @Test fun cancellationAfterEvictionCommitStillAwaitsFileRemoval() = runTest {
        val cache = memoryCache { now }; val removing = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>(); var removed = false
        try {
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            cache.removeEvictedItemFiles = { removing.complete(Unit); release.await(); removed = true }
            val eviction = launch { cache.evictItem(row.itemId) }
            removing.await(); assertTrue(cache.view.value.attachments.isEmpty())
            eviction.cancel(); release.complete(Unit); eviction.join()
            assertTrue(removed)
        } finally { release.complete(Unit); cache.close() }
    }
    @Test fun privateViewerWriteAndEvictionCannotLeaveAResurrectedFile() = runTest {
        val cache = memoryCache { now }; val writing = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>(); var viewerFile = false
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun read(key: String) = byteArrayOf(1)
            override suspend fun removeView(id: String) { viewerFile = false }
        }
        val transport = api { error("cached read only") }
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now }, attachmentStore = store)
            val row = attachmentFixture(); cache.putAttachment(row)
            val opening = async { data.attachments.open(row) { writing.complete(Unit); release.await(); viewerFile = true } }
            writing.await()
            val eviction = async { cache.evictItem(row.itemId) }
            release.complete(Unit); opening.await(); eviction.await()
            assertFalse(viewerFile)
            assertFailsWith<IllegalArgumentException> { data.attachments.open(row) { viewerFile = true } }
        } finally { release.complete(Unit); transport.close(); cache.close() }
    }
    @Test fun fileFailuresPreserveOriginalProblemAndRefetchRemainingIds() = runTest {
        val cache = memoryCache { now }; val gets = mutableListOf<String>()
        val transport = api { req ->
            if (req.url.encodedPath.startsWith("/v1/items/")) { gets += req.url.encodedPath; notFound() }
            else respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a"), itemRead("b"))))
            cache.removeEvictedItemFiles = { throw IllegalStateException("file removal failed") }
            val problem = assertFailsWith<ApiProblem> { data.items.get("a") }
            assertEquals(404, problem.status); assertTrue(problem.suppressedExceptions.isNotEmpty())
            data.sync.refetchByIds(setOf("a", "b"), emptySet())
            assertTrue("/v1/items/b" in gets)
            assertTrue(cache.view.value.items.isEmpty())
        } finally { transport.close(); cache.close() }
    }
}
