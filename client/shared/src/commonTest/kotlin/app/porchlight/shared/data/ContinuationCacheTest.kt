package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlinx.datetime.LocalDate
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class ContinuationCacheTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun observersSeeOnlyCommittedGenerationAndCursor() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
            val seen = mutableListOf<CacheView>()
            val cursors = mutableListOf<Long?>()
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { cache.view.toList(seen) }
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { cache.sync.map { it?.cursor }.toList(cursors) }
            cache.installStaged(emptySnapshot(2).copy(items = listOf(itemRead("new")))) {
                assertEquals(listOf("old"), cache.view.value.items.map { it.id })
                assertEquals(1, cache.sync.value?.cursor)
            }
            assertEquals(listOf<Long?>(1L, 2L), cursors)
            assertEquals(listOf(listOf("old"), listOf("new")), seen.map { it.items.map { row -> row.id } })
            val prior = cache.view.value
            assertFailsWith<IllegalStateException> {
                cache.installStaged(emptySnapshot(3), beforeCommit = { error("promotion failure") })
            }
            assertEquals(prior, cache.view.value)
            cache.initialize() // Reload the database, proving rollback rather than just no flow emission.
            assertEquals(prior, cache.view.value)
        } finally { cache.close() }
    }
    @Test fun agendaDateOnlyExpiresAtMemberLocalEndOfDayAndKeepsUndated() = runTest {
        val at = Instant.parse("2026-10-06T00:30:00Z")
        val cache = memoryCache { at }
        try {
            // Cutoff 2026-04-09T00:30Z. 2026-04-08 ends 2026-04-09T07:00Z in Los Angeles (kept) but 2026-04-09T00:00Z in
            // UTC (would be dropped): only member-local end-of-day expiry retains it.
            val date = LocalDate.parse("2026-04-08")
            assertEquals(Instant.parse("2026-04-09T00:30:00Z"), at - 180.days)
            val member = WireJson.decodeFromString<MemberRead>("""{"id":"self","household_id":"household","display_name":"Self","role":"member","color":"red","initials":"S","tzid":"America/Los_Angeles","tz_source":"manual","version":1,"pending_derivation":false}""")
            cache.install(emptySnapshot(1).copy(members = listOf(member)))
            fun row(id: String, due: LocalDate?) = itemRead("item-$id").copy(agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"household","item_id":"item-$id","derived_at":"2026-10-06T12:00:00Z","member_id":"self","occurrence_id":"$id","due_date":${due?.let { "\"$it\"" } ?: "null"}}"""))
            cache.replaceAgenda(listOf(row("boundary", date), row("old", LocalDate.parse("2020-01-01")), row("undated", null)))
            cache.cleanup(at)
            assertEquals(setOf("boundary", "undated"), cache.view.value.agenda.map { it.agenda!!.occurrenceId }.toSet())
        } finally { cache.close() }
    }
    @Test fun latePreviousSessionCannotPromoteAfterRapidRelogin() = runTest {
        val cache = memoryCache { now }
        var epoch = 1L
        cache.sessionEpoch = { epoch }
        try {
            cache.install(emptySnapshot(1))
            val released = CompletableDeferred<Unit>()
            val started = CompletableDeferred<Unit>()
            val old = async {
                cache.inSession { started.complete(Unit); released.await(); cache.install(emptySnapshot(99)) }
            }
            started.await(); epoch++; cache.clear(); epoch++
            cache.install(emptySnapshot(2))
            released.complete(Unit)
            assertFailsWith<CancellationException> { old.await() }
            assertEquals(2, cache.sync.value?.cursor)
        } finally { cache.close() }
    }
    @Test fun writesReplaceExplicitNullAndPreserveReminderIdentityOnlyForUnchangedOffsets() = runTest {
        val cache = memoryCache { now }
        try {
            val reminder = ReminderRead("identity", "household", "one", 15, now)
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one").copy(address = "old", reminders = listOf(reminder)))))
            val response = itemResponse("one", listOf(15))
            cache.putItem(response)
            assertNull(cache.view.value.items.single().address)
            assertEquals(listOf(reminder), cache.view.value.items.single().reminders)
            cache.putItem(response.copy(reminderOffsets = listOf(30)))
            assertTrue(cache.view.value.items.single().reminders.isEmpty()) // Never expose obsolete or invented IDs.
            assertEquals(listOf(30), cache.itemWrite("one")?.reminderOffsets)
        } finally { cache.close() }
    }
}
fun itemResponse(id: String, offsets: List<Int> = emptyList()): ItemResponse {
    val fields = WireJson.encodeToJsonElement(ItemRead.serializer(), itemRead(id)).jsonObject
    return WireJson.decodeFromJsonElement(ItemResponse.serializer(), kotlinx.serialization.json.JsonObject(fields + mapOf(
        "reminder_offsets" to kotlinx.serialization.json.JsonArray(offsets.map { kotlinx.serialization.json.JsonPrimitive(it) }),
        "conflicts" to kotlinx.serialization.json.JsonArray(emptyList()),
        "address" to kotlinx.serialization.json.JsonNull)))
}
