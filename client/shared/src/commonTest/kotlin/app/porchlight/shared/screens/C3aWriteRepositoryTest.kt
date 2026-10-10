package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.screens.board.boardColumns
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class C3aWriteRepositoryTest {
    @Test fun filteredMoveRelockReplayKeepsNeighboursBodyKeyAndVersionAfterProjectionChanges() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val bodies = mutableListOf<String>()
        val keys = mutableListOf<String?>()
        val versions = mutableListOf<String?>()
        var locked = false
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            bodies += req.body.toByteArray().decodeToString(); keys += req.headers["Idempotency-Key"]; versions += req.headers["If-Match"]
            if (bodies.size == 1) locked = true
            respond(WireJson.encodeToString(OccurrenceResponse.serializer(), occurrenceResponse("mover", "one")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }, beforeCall = { if (locked) throw HouseholdLocked() }), { now })
            data.occurrences.afterWrite = {}
            cache.install(emptySnapshot(1))
            cache.replaceBoard(listOf(card("a", "A"), card("hidden", "B").copy(ownerId = "other"), card("c", "C")))
            val replay = BoardMoveReplay()
            val writer = ScreenWrites { "same-key" }
            writer.submit { data.occurrences.move("mover", 1, BoardDrop("todo", 1), it,
                visible = { projection -> boardColumns(projection, "mine", "self").flatMap { c -> c.cards } }, replay = replay).feedback() }
            assertEquals("same-key", writer.state.value.retryKey)
            cache.replaceBoard(listOf(card("different", "A")))
            locked = false; writer.retry()
            assertEquals(listOf<String?>("same-key", "same-key"), keys)
            assertEquals(listOf<String?>("1", "1"), versions)
            assertEquals(bodies[0], bodies[1])
            val move = WireJson.parseToJsonElement(bodies[0]).jsonObject["board_move"]!!.jsonObject
            assertEquals("a", move["after_id"]!!.jsonPrimitive.content)
            assertEquals("c", move["before_id"]!!.jsonPrimitive.content)
            assertTrue(writer.state.value.saved)
        } finally { api.close(); cache.close() }
    }
}
