package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.serialization.json.*

/** Actual HTTP handlers supply these recordings; the Rust equality test guards every byte of their wire shape. */
class CollaborationFixtureTest {
    private fun recording(name: String) = checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() }
    @Test fun recordedOccurrenceCheckUncheckSurviveTemplateSnapshotAndUseRealMoveNeighbours() = runTest {
        val checked = WireJson.decodeFromString<ListResponse>(recording("list-occurrence-check"))
        val unchecked = WireJson.decodeFromString<ListResponse>(recording("list-occurrence-uncheck"))
        val template = WireJson.decodeFromString<ListResponse>(recording("list-occurrence-template"))
        val other = WireJson.decodeFromString<ListResponse>(recording("list-occurrence-other"))
        val checkEntry = WireJson.decodeFromString<EntryResponse>(recording("entry-occurrence-check"))
        val uncheckEntry = WireJson.decodeFromString<EntryResponse>(recording("entry-occurrence-uncheck"))
        assertTrue(checked.gotIt.isEmpty()); assertNotNull(checked.propertyEntries.first().checkedAt)
        assertNull(unchecked.propertyEntries.first().checkedAt); assertNull(other.propertyEntries.first().checkedAt)
        val cache = memoryCache { assertNotNull(checked.createdAt) }
        var current = template.copy(occurrenceId = checked.occurrenceId, propertyEntries = template.propertyEntries.map { it.copy(occurrenceId = checked.occurrenceId) })
        var moveBody: JsonObject? = null
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            val reply = if (req.method == HttpMethod.Get) {
                assertEquals(checked.occurrenceId, req.url.parameters["occurrence_id"])
                WireJson.encodeToString(ListResponse.serializer(), current)
            } else {
                val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                if (req.method == HttpMethod.Patch) { moveBody = body; WireJson.encodeToString(EntryResponse.serializer(), checkEntry) }
                else {
                    assertEquals(checked.occurrenceId, body["occurrence_id"]?.jsonPrimitive?.content)
                    val checking = req.url.encodedPath.endsWith("/check")
                    current = if (checking) checked else unchecked
                    WireJson.encodeToString(EntryResponse.serializer(), if (checking) checkEntry else uncheckEntry)
                }
            }
            respond(reply, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        try {
            cache.putList(current)
            val repo = ListRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            repo.check(checkEntry.id, 1, true, checked.occurrenceId)
            assertEquals(checked, cache.view.value.lists.single())
            cache.putList(other); cache.putList(template)
            cache.install(emptySnapshot(8).copy(items = listOf(itemRead(checked.itemId!!)), lists = listOf(template),
                occurrences = listOf(occurrenceRead(checked.occurrenceId!!, checked.itemId), occurrenceRead(other.occurrenceId!!, other.itemId!!))))
            assertEquals(3, cache.view.value.lists.size)
            assertNotNull(cache.view.value.lists.single { it.occurrenceId == checked.occurrenceId }.propertyEntries.first().checkedAt)
            repo.move(checked, checkEntry, 1)
            assertEquals(checked.propertyEntries.last().id, moveBody?.get("after_id")?.jsonPrimitive?.content)
            assertEquals(JsonNull, moveBody?.get("before_id"))
            repo.check(checkEntry.id, checkEntry.version, false, checked.occurrenceId)
            assertEquals(unchecked, cache.view.value.lists.single { it.occurrenceId == checked.occurrenceId })
        } finally { cache.close(); transport.close() }
    }
    @Test fun serverCollaborationShapesDecodeAndCache() = runTest {
        val list = WireJson.decodeFromString<ListResponse>(recording("list"))
        val comment = WireJson.decodeFromString<Comment>(recording("comment"))
        val attachment = WireJson.decodeFromString<Attachment>(recording("attachment"))
        assertEquals("Groceries", list.name)
        assertEquals("Oat milk", list.propertyEntries.single().label)
        assertNull(list.propertyEntries.single().checkedAt)
        assertEquals("Bread", list.gotIt.single().label)
        assertNotNull(list.gotIt.single().checkedAt)
        assertEquals("Recorded comment", comment.body)
        assertEquals("notes.txt", attachment.originalName)
        val snapshot = WireJson.decodeFromString<SnapshotRead>(recording("snapshot"))
        val cache = memoryCache { Instant.parse("2026-10-06T12:00:00Z") }
        try {
            cache.install(snapshot)
            assertEquals(snapshot.lists, cache.view.value.lists)
            assertEquals(snapshot.comments, cache.view.value.comments)
            assertEquals(snapshot.attachments, cache.view.value.attachments)
        } finally { cache.close() }
    }
}
