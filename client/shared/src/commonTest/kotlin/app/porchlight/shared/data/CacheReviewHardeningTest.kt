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

class CacheReviewHardeningTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun snapshotsAndMemberCollectionsAuthoritativelyReplaceLowerVersions() = runTest {
        val cache = memoryCache { now }
        try {
            val member = MemberRead(false, "self", "household", "Self", "user", "#FABD2F", "S", "UTC", "manual", 3)
            val snapshot = emptySnapshot(30).copy(items = listOf(itemRead("one").copy(version = 3)), occurrences = listOf(occurrenceRead("occ", "one").copy(version = 3)), members = listOf(member), memberSettings = emptySnapshot(1).memberSettings.copy(version = 3))
            cache.install(snapshot)
            cache.putItem(itemResponse("one").copy(version = 5))
            cache.putOccurrence(occurrenceResponse("occ", "one").copy(version = 5))
            cache.putMember(member.copy(version = 5))
            cache.replaceMembers(listOf(member))
            assertEquals(3, cache.view.value.members.single().version)
            cache.putSettings(snapshot.memberSettings.copy(version = 5))
            val mark = cache.snapshotMark()
            assertEquals(30, mark.cursor)
            cache.putItem(itemResponse("one").copy(version = 6))
            // Cursor regression is restore/rollback: even post-request receipts belong to the old epoch.
            cache.installStaged(snapshot.copy(cursor = 20), receivedAfter = mark.receipts, requestCursor = mark.cursor)
            assertEquals(3, cache.view.value.items.single().version)
            assertNull(cache.itemWrite("one"))
            assertEquals(3, cache.view.value.occurrences.single().version)
            assertEquals(3, cache.view.value.members.single().version)
            assertEquals(3, cache.view.value.settings!!.version)
            cache.putItem(itemResponse("one").copy(version = 4, title = "Restored write"))
            cache.putOccurrence(occurrenceResponse("occ", "one").copy(version = 4))
            assertEquals("Restored write", cache.view.value.items.single().title)
            assertEquals(4, cache.view.value.occurrences.single().version)
            cache.install(snapshot.copy(cursor = 21)) // ordinary resnapshot is authoritative too
            assertEquals(3, cache.view.value.items.single().version)
        } finally { cache.close() }
    }
    @Test fun individualResponsesStillRejectOlderVersionsAndDeletesWithinABatch() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            cache.putItems(listOf(itemResponse("one").copy(version = 3, title = "New"), itemResponse("one")))
            cache.putItem(itemResponse("one").copy(version = 2, deletedAt = now))
            assertEquals(3, cache.view.value.items.single().version)
            assertEquals("New", cache.view.value.items.single().title)
            assertEquals(3, cache.itemWrite("one")!!.version)
        } finally { cache.close() }
    }
    @Test fun itemAndOccurrenceReceiptsAfterRequestReapplyTogether() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one")))
            cache.install(snapshot)
            val mark = cache.receiptMark()
            cache.putItem(itemResponse("one").copy(version = 3, title = "New"))
            cache.putOccurrence(occurrenceResponse("occ", "one").copy(version = 2, status = OccurrenceStatus.doing))
            cache.installStaged(snapshot.copy(cursor = 2), receivedAfter = mark)
            assertEquals(3, cache.view.value.items.single().version)
            assertEquals("New", cache.view.value.items.single().title)
            assertEquals(2, cache.view.value.occurrences.single().version)
        } finally { cache.close() }
    }
    @Test fun fixedOverridesUseItemZoneEarlierFoldAndNextValidGapWithPositiveSpan() {
        val item = itemRead("one").copy(kind = "appointment", tzid = "America/New_York")
        val gap = occurrenceRead("gap", "one").copy(isDetached = true, overrideLocalStart = "2026-03-08T02:10:00", overrideLocalEnd = "2026-03-08T02:40:00").effective(item)
        assertEquals(Instant.parse("2026-03-08T07:00:00Z"), gap.startsAt)
        assertEquals(Instant.parse("2026-03-08T07:30:00Z"), gap.endsAt)
        val fold = occurrenceRead("fold", "one").copy(overrideLocalDue = "2026-11-01T01:30:00").effective(item)
        assertEquals(Instant.parse("2026-11-01T05:30:00Z"), fold.dueAt)
        val floating = occurrenceRead("floating", "one").copy(overrideLocalDue = "2026-11-01T01:30:00").effective(item.copy(timeMode = "floating"))
        assertEquals("2026-11-01T01:30:00", floating.localDue)
        assertNull(floating.dueAt)
    }
    @Test fun dayMembershipUsesLocalDaysAcrossDSTAndOverlappingPartialWindows() {
        val row = agenda("day", member = "other", due = LocalDate.parse("2026-11-01")).agenda!!
        val zone = TimeZone.of("America/New_York")
        assertTrue(provablyInAgendaWindow(row, Instant.parse("2026-11-01T04:00:00Z"), Instant.parse("2026-11-02T05:00:00Z"), zone))
        assertTrue(provablyInAgendaWindow(row, Instant.parse("2026-11-02T04:59:00Z"), Instant.parse("2026-11-02T05:01:00Z"), zone))
        assertFalse(provablyInAgendaWindow(row, Instant.parse("2026-11-02T05:00:00Z"), Instant.parse("2026-11-03T05:00:00Z"), zone))
    }
    @Test fun authoritativeEvictionCannotBeUndoneByAnAlreadyRequestedSnapshot() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead(collabId("item"))), comments = listOf(commentFixture()), attachments = listOf(attachmentFixture()), lists = listOf(listFixture().copy(itemId = collabId("item"))))
            cache.install(snapshot)
            val mark = cache.receiptMark()
            cache.evictItem(collabId("item"))
            cache.installStaged(snapshot.copy(cursor = 2), receivedAfter = mark)
            assertTrue(cache.view.value.items.isEmpty()); assertTrue(cache.view.value.comments.isEmpty()); assertTrue(cache.view.value.attachments.isEmpty()); assertTrue(cache.view.value.lists.isEmpty())
        } finally { cache.close() }
    }
    @Test fun cachedDownloadCannotReturnAfterAuthoritativeVisibilityEviction() = runTest {
        val cache = memoryCache { now }; val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun read(key: String): ByteArray { started.complete(Unit); release.await(); return byteArrayOf(1) }
        }
        val api = KtorApiTransport(HttpClient(MockEngine { error("Cached bytes must not require network") }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }, attachmentStore = store)
            val row = attachmentFixture(); cache.putAttachment(row)
            val download = async { runCatching { data.attachments.download(row) } }
            started.await(); cache.evictItem(row.itemId); release.complete(Unit)
            assertIs<CancellationException>(download.await().exceptionOrNull())
        } finally { release.complete(Unit); api.close(); cache.close() }
    }
    @Test fun stepUpRequiredDoesNotMeanVisibilityWasRevoked() {
        assertFalse(ApiProblem("step-up-required", 403, "Unlock").deniesVisibility())
        assertFalse(ApiProblem("invalid-response", 502, "Retry").deniesVisibility())
        assertTrue(ApiProblem("forbidden", 403, "Forbidden").deniesVisibility())
    }
}
