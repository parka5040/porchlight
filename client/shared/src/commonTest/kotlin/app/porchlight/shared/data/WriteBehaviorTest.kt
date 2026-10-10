package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*

class WriteBehaviorTest {
    @Test fun transientConflictRetriesSameKeyAndVersionButNotVersionConflict() = runTest {
        var calls = 0
        val keys = mutableListOf<String?>()
        val waits = mutableListOf<Long>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            calls++; keys += req.headers["Idempotency-Key"]
            assertEquals("7", req.headers["If-Match"])
            if (calls == 1) respond("""{"type":"urn:porchlight:problem:conflict","status":409,"title":"Busy"}""", HttpStatusCode.Conflict,
                headersOf(HttpHeaders.ContentType to listOf("application/problem+json"), HttpHeaders.RetryAfter to listOf("2")))
            else respond("""{"type":"urn:porchlight:problem:version-conflict","status":409,"title":"Changed","current":{"version":9}}""", HttpStatusCode.Conflict,
                headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val coordinator = WriteCoordinator(newKey = { "stable-operation" }, wait = { waits += it })
        val result = coordinator.execute { key -> transport.request("PATCH", "/v1/items/example", buildJsonObject {}, idempotencyKey = key, version = 7) }
        assertIs<WriteResult.VersionConflict>(result)
        assertEquals(9, result.current?.jsonObject?.get("version")?.jsonPrimitive?.int)
        assertEquals(listOf<String?>("stable-operation", "stable-operation"), keys)
        assertEquals(listOf(2000L), waits)
        transport.close()
    }
    @Test fun onlyExplicitTransientContentionRetriesAndTheAttemptsAreBounded() = runTest {
        val waits = mutableListOf<Long>()
        val coordinator = WriteCoordinator(newKey = { "key" }, wait = { waits += it })
        var calls = 0
        // 409 conflict without Retry-After is sealed: surfaced at once, never replayed.
        assertEquals("conflict", assertFailsWith<ApiProblem> {
            coordinator.execute<Unit> { calls++; throw ApiProblem("conflict", 409, "Busy") }
        }.type)
        assertEquals(1, calls)
        // An ambiguous IO failure may have committed: never replayed either.
        calls = 0
        assertFailsWith<ConnectFailure> { coordinator.execute<Unit> { calls++; throw ConnectFailure() } }
        assertEquals(1, calls)
        // Persistent transient contention stops after maxAttempts (3), with the same key each time.
        calls = 0
        val keys = mutableListOf<String>()
        assertFailsWith<ApiProblem> {
            coordinator.execute<Unit> { key -> calls++; keys += key; throw ApiProblem("conflict", 409, "Busy", retryAfterMillis = 100) }
        }
        assertEquals(3, calls); assertEquals(listOf("key", "key", "key"), keys)
        assertEquals(listOf(250L, 500L), waits) // backoff floor dominates the 100 ms Retry-After
    }
    @Test fun offlineDoesNotQueueOrCall() = runTest {
        val c = WriteCoordinator(online = { false })
        assertFailsWith<OfflineWrite> { c.execute { error("must not call") } }
    }
    @Test fun neighbourSelectionUsesVisibleProjectionAndWireDirection() {
        val rows = listOf(card("above", "A"), card("mover", "B"), card("below", "C"))
        assertEquals(BoardNeighbours("above", "below"), boardNeighbours(rows, "mover", BoardDrop("todo", 1)))
        assertEquals(BoardNeighbours(null, "above"), boardNeighbours(rows, "mover", BoardDrop("todo", 0)))
        assertEquals(BoardNeighbours("below", null), boardNeighbours(rows, "mover", BoardDrop("todo", 9)))
    }
    @Test fun patchPresencePreservesExplicitNullAndLocalWallTime() {
        val patch = ItemEdit(PatchRequestPlaceholder()).clear("subject_member_id").local("local_due", "2026-10-06T14:00:00")
        assertEquals(JsonNull, patch.body["subject_member_id"])
        assertEquals("2026-10-06T14:00:00", patch.body["local_due"]?.jsonPrimitive?.content)
        assertFalse("notes" in patch.body)
    }
}
fun PatchRequestPlaceholder() = buildJsonObject {}
fun card(id: String, position: String): ItemRead = itemRead("item-$id").copy(occurrence = occurrenceRead(id, "item-$id", position))
fun itemRead(id: String, pending: Boolean = false): ItemRead = WireJson.decodeFromString("""{"id":"$id","household_id":"household","title":"Title","notes":"","priority":0,"owner_id":"self","visibility":"household","kind":"todo","time_mode":"fixed","all_day":false,"buffer_before_min":0,"buffer_after_min":0,"missed_policy":"carry_over","version":1,"created_at":"2026-10-06T12:00:00Z","updated_at":"2026-10-06T12:00:00Z","assignees":[],"viewers":[],"reminders":[],"capabilities":["view","edit"],"pending_derivation":$pending}""")
fun occurrenceRead(id: String, item: String, position: String = "A"): OccurrenceRead = WireJson.decodeFromString("""{"id":"$id","household_id":"household","item_id":"$item","seq":1,"is_detached":false,"disposition":"open","status":"todo","board_position":"$position","board_moved":false,"version":1,"created_at":"2026-10-06T12:00:00Z","updated_at":"2026-10-06T12:00:00Z"}""")
