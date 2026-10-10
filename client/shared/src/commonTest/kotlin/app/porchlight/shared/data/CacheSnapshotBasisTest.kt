package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

/** N-1 (merge with C4): R81's restore check uses the cursor installed when the snapshot was REQUESTED. C4's stream
 * acks advance the live cursor while a snapshot loads, which must not make an ordinary resnapshot look like a restore. */
class CacheSnapshotBasisTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private val member = MemberRead(false, "self", "household", "Self", "user", "#FABD2F", "S", "UTC", "manual", 3)
    private fun snapshot(cursor: Long) = emptySnapshot(cursor).copy(items = listOf(itemRead("one"), itemRead("two"), itemRead("three")),
        members = listOf(member))
    private fun json(text: String) = WireJson.parseToJsonElement(text).jsonObject

    @Test fun streamAckDuringSnapshotLoadKeepsPostRequestReceiptsButRestoreDropsThem() = runTest {
        val cache = memoryCache { now }
        var served: SnapshotRead = snapshot(60)
        var whileLoading: suspend () -> Unit = {}
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject =
                when (path.substringBefore('?')) {
                    "/v1/snapshot" -> { whileLoading(); WireJson.encodeToJsonElement(served).jsonObject }
                    "/v1/board", "/v1/agenda" -> json("""{"items":[],"next_cursor":null}""")
                    else -> error("unexpected $method $path")
                }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.install(snapshot(51))
            whileLoading = {
                // Responses received after the request: an own write, a delete and a visibility denial.
                cache.putItem(itemResponse("one").copy(version = 5, title = "Own write"))
                cache.putItem(itemResponse("two").copy(version = 2, deletedAt = now))
                cache.evictItem("three")
                cache.putMember(member.copy(version = 5, displayName = "Renamed"))
                // A narrow C4 tickle (occurrence/notification) is handled and acked meanwhile: above the snapshot's cursor.
                cache.acknowledgeCursor(70)
            }
            data.sync.refresh()
            assertEquals(60, cache.sync.value?.cursor)
            assertEquals(listOf("one"), cache.view.value.items.map { it.id })
            assertEquals(5, cache.view.value.items.single().version)
            assertEquals("Own write", cache.view.value.items.single().title)
            assertEquals("Renamed", cache.view.value.members.single().displayName)

            // A real restore: the snapshot predates the cursor installed when it was requested. Post-request receipts
            // belong to the old epoch and are dropped (R81), even without any stream ack.
            served = snapshot(40)
            whileLoading = { cache.putItem(itemResponse("one").copy(version = 6, title = "Old epoch")) }
            data.sync.refresh()
            assertEquals(40, cache.sync.value?.cursor)
            assertEquals(listOf("one", "three", "two"), cache.view.value.items.map { it.id }.sorted())
            assertTrue(cache.view.value.items.all { it.version == 1 })
            assertEquals(3, cache.view.value.members.single().version)
        } finally { data.close() }
    }

    /** n-2: C4's item tickle (`refetchByIds`) refreshes the member collection while an older snapshot loads. That
     * collection install must not discard the member write receipt the snapshot install still needs. */
    @Test fun memberCollectionRefreshDuringSnapshotLoadKeepsMemberWriteReceipt() = runTest {
        val cache = memoryCache { now }
        var whileLoading: suspend () -> Unit = {}
        lateinit var data: HouseholdData
        val renamed = member.copy(version = 5, displayName = "Renamed")
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject =
                when (path.substringBefore('?')) {
                    "/v1/snapshot" -> { whileLoading(); WireJson.encodeToJsonElement(snapshot(60)).jsonObject }
                    "/v1/members" -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(renamed)))); put("next_cursor", JsonNull) }
                    "/v1/items/one" -> WireJson.encodeToJsonElement(itemResponse("one")).jsonObject
                    "/v1/board", "/v1/agenda", "/v1/items/one/occurrences" -> json("""{"items":[],"next_cursor":null}""")
                    else -> error("unexpected $method $path")
                }
        }
        data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.install(snapshot(51))
            whileLoading = {
                cache.putMember(renamed) // own member PATCH response, after the snapshot request
                data.sync.refetchByIds(itemIds = setOf("one")) // C4 item tickle: item, occurrences, member collection
                assertEquals("Renamed", cache.view.value.members.single().displayName)
            }
            data.sync.refresh() // the snapshot was read before the PATCH: member v3
            assertEquals(5, cache.view.value.members.single().version)
            assertEquals("Renamed", cache.view.value.members.single().displayName)
        } finally { data.close() }
    }
}
