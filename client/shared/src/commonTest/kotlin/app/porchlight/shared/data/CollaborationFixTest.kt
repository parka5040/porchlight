package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class CollaborationFixTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun scoped(scope: String) = listFixture(scope).let { list -> list.copy(propertyEntries = list.propertyEntries.map { it.copy(occurrenceId = list.occurrenceId) }) }

    @Test fun snapshotAndTemplateReadsPreserveDistinctOccurrenceChecksAndTicklesRefetchAllScopes() = runTest {
        val cache = memoryCache { now }
        val queries = mutableListOf<String?>()
        val one = scoped("one").let { it.copy(propertyEntries = it.propertyEntries.mapIndexed { i, e -> if (i == 0) e.copy(checkedAt = now, checkedBy = collabId("self")) else e }) }
        val two = scoped("two")
        val template = one.copy(occurrenceId = null, propertyEntries = one.propertyEntries.map { it.copy(occurrenceId = null, checkedAt = null, checkedBy = null) })
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            val scope = req.url.parameters["occurrence_id"]; queries += scope
            respond(WireJson.encodeToString(ListResponse.serializer(), when (scope) { one.occurrenceId -> one; two.occurrenceId -> two; else -> template }), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        try {
            cache.putList(one); cache.putList(two); cache.putList(template)
            val snapshot = emptySnapshot(5).copy(items = listOf(itemRead(collabId("item"))), lists = listOf(template),
                occurrences = listOf(occurrenceRead(one.occurrenceId!!, collabId("item")), occurrenceRead(two.occurrenceId!!, collabId("item"))))
            cache.install(snapshot)
            assertEquals(3, cache.view.value.lists.size)
            assertNotNull(cache.view.value.lists.single { it.occurrenceId == one.occurrenceId }.propertyEntries.first().checkedAt)
            assertNull(cache.view.value.lists.single { it.occurrenceId == two.occurrenceId }.propertyEntries.first().checkedAt)
            ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator()).refetchByIds(setOf(one.id))
            assertEquals(setOf(null, one.occurrenceId, two.occurrenceId), queries.toSet())
            cache.evictItem(collabId("item")); assertTrue(cache.view.value.lists.isEmpty())
        } finally { cache.close(); transport.close() }
    }

    @Test fun perOccurrenceCheckUncheckAndCheckedMoveUseEntriesProjection() = runTest {
        val cache = memoryCache { now }
        var current = scoped("one")
        val bodies = mutableListOf<JsonObject>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            if (req.method == HttpMethod.Get) respond(WireJson.encodeToString(ListResponse.serializer(), current), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            else {
                val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject; bodies += body
                val entry = current.propertyEntries.first().let { e -> when {
                    req.url.encodedPath.endsWith("/uncheck") -> e.copy(version = e.version + 1, checkedAt = null, checkedBy = null)
                    req.url.encodedPath.endsWith("/check") -> e.copy(version = e.version + 1, checkedAt = now, checkedBy = collabId("self"))
                    else -> e.copy(version = e.version + 1)
                } }
                current = current.copy(propertyEntries = listOf(entry) + current.propertyEntries.drop(1))
                respond(WireJson.encodeToString(EntryResponse.serializer(), entry), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        try {
            cache.putList(current)
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            assertIs<WriteResult.Success<EntryResponse>>(repo.check(current.propertyEntries.first().id, 1, true, current.occurrenceId))
            assertTrue(cache.view.value.lists.single().gotIt.isEmpty())
            val checked = current.propertyEntries.first()
            cache.putEntry(checked); assertTrue(cache.view.value.lists.single().gotIt.isEmpty())
            repo.move(current, checked, 1)
            assertEquals(current.propertyEntries.last().id, bodies.last()["after_id"]?.jsonPrimitive?.content)
            assertEquals(JsonNull, bodies.last()["before_id"])
            assertIs<WriteResult.Success<EntryResponse>>(repo.check(checked.id, current.propertyEntries.first().version, false, current.occurrenceId))
            assertNull(cache.view.value.lists.single().propertyEntries.first().checkedAt)
            assertEquals(current.occurrenceId, bodies.first()["occurrence_id"]?.jsonPrimitive?.content)
        } finally { cache.close(); transport.close() }
    }

    @Test fun collectionRefreshDeletionsFenceOlderSnapshotsAndRemoveAllListScopes() = runTest {
        val cache = memoryCache { now }
        try {
            val template = scoped("one").copy(occurrenceId = null)
            val old = emptySnapshot(1).copy(items = listOf(itemRead(collabId("item"))), lists = listOf(template), comments = listOf(commentFixture()))
            cache.install(old); cache.putList(scoped("one")); cache.putList(scoped("two"))
            val mark = cache.receiptMark()
            cache.replaceLists(null, emptyList()); cache.replaceComments(collabId("item"), emptyList())
            assertTrue(cache.view.value.lists.isEmpty()); assertTrue(cache.view.value.comments.isEmpty())
            cache.installStaged(old, receivedAfter = mark)
            assertTrue(cache.view.value.lists.isEmpty()); assertTrue(cache.view.value.comments.isEmpty())
        } finally { cache.close() }
    }
    @Test fun mentionHighlightHonoursLongestCaseQuotedAndWordBoundaries() {
        val members = listOf(
            MemberRead(false, collabId("self"), collabId("home"), "Al", "user", "#FABD2F", "A", "UTC", "manual", 1),
            MemberRead(false, collabId("a"), collabId("home"), "Alfred", "user", "#FABD2F", "A", "UTC", "manual", 1),
            MemberRead(false, collabId("b"), collabId("home"), "Al Smith", "user", "#FABD2F", "A", "UTC", "manual", 1)
        )
        val body = "@Al @Alfred @al @Al_name x@Al @Al-foo @Al Smith @\"Al\""
        val c = commentFixture().copy(body = body, mentions = listOf(collabId("self"), collabId("b")))
        assertEquals(listOf("@Al", "@al", "@Al Smith", "@\"Al\""), mentionRanges(c, members).map { body.substring(it) })
    }
    @Test fun manyThumbnailsBoundSimultaneousDownloads() = runTest {
        val cache = memoryCache { now }
        val release = CompletableDeferred<Unit>(); val started = CompletableDeferred<Unit>()
        var active = 0; var maximum = 0
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = error("no JSON")
            override suspend fun download(path: String, session: String?): ByteArray {
                active++; maximum = maxOf(maximum, active); if (active == 2) started.complete(Unit)
                try { release.await(); return byteArrayOf(1, 2, 3) } finally { active-- }
            }
        }
        try {
            val repo = AttachmentRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            val rows = (1..20).map { attachmentFixture().copy(id = "01999999-0000-7000-8000-${it.toString().padStart(12, '0')}", hasThumbnail = true) }
            rows.forEach { cache.putAttachment(it) }
            val downloads = rows.map { async { repo.download(it, true) } }
            started.await(); assertEquals(2, maximum); release.complete(Unit)
            downloads.awaitAll(); assertEquals(2, maximum)
        } finally { release.complete(Unit); cache.close() }
    }
}
