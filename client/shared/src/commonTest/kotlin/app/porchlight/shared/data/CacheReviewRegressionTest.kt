package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.screens.itemTimeLabel
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlinx.serialization.encodeToString
import kotlin.test.*
import kotlin.time.Instant

class CacheReviewRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun MockRequestHandleScope.json(body: String) = respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
    private fun MockRequestHandleScope.denied(status: Int) = respond("""{"type":"urn:porchlight:problem:${if (status == 404) "not-found" else "forbidden"}","status":$status,"title":"Unavailable"}""", HttpStatusCode.fromValue(status), headersOf(HttpHeaders.ContentType, "application/problem+json"))

    @Test fun d01CleanupUsesRescheduledAnchorsNotGeneratedDates() = runTest {
        val cache = memoryCache { now }
        try {
            val item = itemRead("one").copy(tzid = "America/New_York")
            val rows = listOf(
                occurrenceRead("date", "one").copy(isDetached = true, dueDate = LocalDate.parse("2025-01-01"), overrideDueDate = LocalDate.parse("2026-10-06")),
                occurrenceRead("fixed", "one").copy(isDetached = true, dueAt = Instant.parse("2025-01-01T12:00:00Z"), overrideLocalDue = "2026-10-06T08:00:00"),
                occurrenceRead("old", "one").copy(dueDate = LocalDate.parse("2025-01-01")),
                occurrenceRead("undated", "one"))
            cache.install(emptySnapshot(1).copy(items = listOf(item), occurrences = rows))
            assertEquals(setOf("date", "fixed", "undated"), cache.view.value.occurrences.map { it.id }.toSet())
            cache.cleanup()
            assertEquals(3, cache.view.value.occurrences.size)
        } finally { cache.close() }
    }

    @Test fun d03DayRefreshReplacesDateOnlyRowsForRequestedMember() = runTest {
        val cache = memoryCache { now }
        val api = transport { json(WireJson.encodeToString(ItemReadPage(emptyList(), null))) }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
            val self = MemberRead(false, "other", "household", "Other", "user", "#FABD2F", "O", "America/New_York", "manual", 1)
            cache.install(emptySnapshot(1).copy(members = listOf(self), items = listOf(itemRead("one"))))
            cache.replaceAgenda(listOf(agenda("gone", "one", "other", LocalDate.parse("2026-10-06")),
                agenda("tomorrow", "one", "other", LocalDate.parse("2026-10-07")),
                agenda("mine", "one", "self", LocalDate.parse("2026-10-06"))))
            data.agenda.refresh(Instant.parse("2026-10-06T04:00:00Z"), Instant.parse("2026-10-07T04:00:00Z"), "other")
            assertEquals(setOf("tomorrow", "mine"), cache.view.value.agenda.map { it.agenda!!.occurrenceId }.toSet())
        } finally { api.close(); cache.close() }
    }

    @Test fun d06SnapshotPresentationAndEditorUseEffectiveFixedAndDateOnlyAnchors() = runTest {
        val cache = memoryCache { now }
        try {
            val o = occurrenceRead("fixed", "one").copy(isDetached = true, startsAt = Instant.parse("2026-10-05T14:00:00Z"), endsAt = Instant.parse("2026-10-05T15:00:00Z"), overrideLocalStart = "2026-10-06T10:00:00", overrideLocalEnd = "2026-10-06T11:00:00")
            val item = itemRead("one").copy(kind = "appointment", tzid = "America/New_York", occurrence = o)
            val effective = o.copy(startsAt = Instant.parse("2026-10-06T14:00:00Z"), endsAt = Instant.parse("2026-10-06T15:00:00Z"))
            assertEquals(itemTimeLabel(item.copy(occurrence = effective), TimeZone.UTC), itemTimeLabel(item, TimeZone.UTC))
            assertEquals(effective.startsAt.toString(), scopeSeed(item, EditorScope.ThisOccurrence).startsAt)
            assertEquals(effective.endsAt.toString(), scopeSeed(item, EditorScope.ThisOccurrence).endsAt)
            val todo = occurrenceRead("todo", "two").copy(isDetached = true, dueDate = LocalDate.parse("2026-10-05"), overrideDueDate = LocalDate.parse("2026-10-06"))
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("two").copy(ownerId = "other")), occurrences = listOf(todo)))
            val groups = todaySections(todayRows(cache.view.value, "self"), defaultTodaySettings.copy(scope = SettingsScope.household), "self", TimeZone.UTC, now)
            assertEquals(listOf("todo"), groups.single { it.kind == TodaySection.today }.rows.map { it.occurrence!!.id })
            assertTrue(groups.single { it.kind == TodaySection.overdue }.rows.isEmpty())
        } finally { cache.close() }
    }

    @Test fun d07AuthoritativeItemFailureEvictsButOfflineKeepsContent() = runTest {
        for (status in listOf(404, 403, 401, 503)) {
            val cache = memoryCache { now }
            val api = transport { denied(status) }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
                cache.replaceBoard(listOf(itemRead("one").copy(occurrence = occurrenceRead("occ", "one"))))
                assertFailsWith<ApiProblem> { data.items.get("one") }
                assertEquals(status == 503 || status == 401, cache.view.value.items.isNotEmpty(), "status $status")
                assertEquals(status == 503 || status == 401, cache.view.value.board.isNotEmpty(), "status $status")
                assertEquals(status == 503 || status == 401, cache.view.value.occurrences.isNotEmpty(), "status $status")
            } finally { api.close(); cache.close() }
        }
    }

    @Test fun d08DelayedOlderGetCannotOverwriteNewerVersion() = runTest {
        val cache = memoryCache { now }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val api = transport { started.complete(Unit); release.await(); json(WireJson.encodeToString(itemResponse("one"))) }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            val request = async { data.items.get("one") }; started.await()
            cache.putItem(itemResponse("one").copy(title = "New", version = 2))
            release.complete(Unit); request.await()
            assertEquals("New", cache.view.value.items.single().title)
            assertEquals(2, cache.itemWrite("one")!!.version)
            cache.putOccurrence(occurrenceResponse("occ", "one").copy(version = 2, status = OccurrenceStatus.doing))
            cache.putOccurrence(occurrenceResponse("occ", "one"))
            assertEquals(2, cache.view.value.occurrences.single().version)
            val member = MemberRead(false, "self", "household", "Self", "user", "#FABD2F", "S", "UTC", "manual", 1)
            cache.putMember(member.copy(version = 2, displayName = "New")); cache.putMember(member)
            assertEquals(2, cache.view.value.members.single().version)
            val settings = cache.view.value.settings!!
            cache.putSettings(settings.copy(version = 2)); cache.putSettings(settings)
            assertEquals(2, cache.view.value.settings!!.version)
        } finally { release.complete(Unit); api.close(); cache.close() }
    }

    @Test fun c501CommentAndAttachmentDenialsEvictAllContentAndFilesBeforeReturning() = runTest {
        for (endpoint in listOf("comment", "attachment")) for (status in listOf(404, 403)) {
            val cache = memoryCache { now }
            val keys = mutableSetOf<String>(); val views = mutableSetOf<String>()
            val store = object : AttachmentStore {
                override suspend fun read(key: String): ByteArray? = null
                override suspend fun write(key: String, bytes: ByteArray) { keys += key }
                override suspend fun clear() { keys.clear(); views.clear() }
                override suspend fun remove(key: String) { keys -= key }
                override suspend fun removeView(id: String) { views -= id }
                override suspend fun cleanup(before: Instant) = Unit
            }
            val api = transport { denied(status) }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now }, attachmentStore = store)
                val row = attachmentFixture().copy(hasThumbnail = true)
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), occurrences = listOf(occurrenceRead("occ", row.itemId)), comments = listOf(commentFixture()), attachments = listOf(row)))
                keys += "0-${row.id}-${row.version}-file"; keys += "0-${row.id}-${row.version}-thumb"; views += row.id
                assertFailsWith<ApiProblem> { if (endpoint == "comment") data.comments.refresh(row.itemId) else data.attachments.download(row) }
                assertTrue(cache.view.value.comments.isEmpty(), "$endpoint $status comments")
                assertTrue(cache.view.value.attachments.isEmpty(), "$endpoint $status attachments")
                assertTrue(cache.view.value.items.isEmpty(), "$endpoint $status parent")
                assertTrue(keys.isEmpty(), "$endpoint $status files"); assertTrue(views.isEmpty(), "$endpoint $status views")
                assertFailsWith<IllegalArgumentException> { data.attachments.download(row) }
            } finally { api.close(); cache.close() }
        }
    }
}
