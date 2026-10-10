package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.LocalDate
import kotlinx.serialization.encodeToString
import kotlin.test.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.hours
import kotlin.time.Instant

/** R59 (snapshot + projections switch together; writes never wipe projections) and I-2 (windowed occurrence refetch). */
class CacheConsistencyTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    private fun MockRequestHandleScope.json(body: String) = respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
    private fun page(vararg rows: ItemRead) = WireJson.encodeToString(ItemReadPage(rows.toList(), null))
    private fun boardRow(item: String, occurrence: String, position: String = "A", status: String = "todo") =
        itemRead(item).copy(occurrence = occurrenceRead(occurrence, item, position).copy(status = status))

    @Test fun snapshotProjectionFailuresKeepEntirePreviousGeneration() = runTest {
        for (failedPath in listOf("/v1/board", "/v1/agenda")) {
            val cache = memoryCache { now }
            val api = transport { req ->
                if (req.url.encodedPath == failedPath) throw ConnectFailure()
                if (req.url.encodedPath == "/v1/snapshot") json(WireJson.encodeToString(emptySnapshot(2).copy(items = listOf(itemRead("new")))))
                else json(page(card("new", "A")))
            }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
                cache.replaceBoard(listOf(card("old", "A"))); cache.replaceAgenda(listOf(agenda("old")))
                val prior = cache.view.value
                assertFailsWith<SyncUnavailable> { data.sync.refresh() }
                assertIs<SyncStatus.Failed>(data.sync.status.value)
                assertEquals(prior, cache.view.value)
                cache.initialize(); assertEquals(prior, cache.view.value)
            } finally { api.close(); cache.close() }
        }
    }
    @Test fun snapshotStagesBothProjectionsBeforePublishing() = runTest {
        val cache = memoryCache { now }
        val agendaStarted = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/snapshot" -> json(WireJson.encodeToString(emptySnapshot(2).copy(items = listOf(itemRead("new")))))
                "/v1/board" -> json(page(card("new", "A")))
                else -> { agendaStarted.complete(Unit); release.await(); json(page(agenda("new"))) }
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
            cache.replaceBoard(listOf(card("old", "A"))); cache.replaceAgenda(listOf(agenda("old")))
            val prior = cache.view.value
            val refresh = async { data.sync.refresh() }
            agendaStarted.await()
            val whileFetching = cache.view.value
            release.complete(Unit); refresh.await()
            assertEquals(prior, whileFetching)
            assertEquals(2L, cache.sync.value?.cursor)
            assertEquals(listOf("new"), cache.view.value.items.map { it.id })
            assertEquals(listOf("new"), cache.view.value.board.map { it.occurrence!!.id })
            assertEquals(listOf("new"), cache.view.value.agenda.map { it.agenda!!.occurrenceId })
        } finally { api.close(); cache.close() }
    }
    @Test fun itemWriteFollowUpFailureNeverWipesProjections() = runTest {
        val cache = memoryCache { now }
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/items/one" -> json(WireJson.encodeToString(itemResponse("one").copy(title = "Renamed", version = 2)))
                "/v1/snapshot" -> json(WireJson.encodeToString(emptySnapshot(2).copy(items = listOf(itemRead("one").copy(title = "Renamed", version = 2)))))
                "/v1/board" -> throw ConnectFailure()
                else -> json(page(agenda("occ", item = "one")))
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            cache.replaceBoard(listOf(boardRow("one", "occ"))); cache.replaceAgenda(listOf(agenda("occ", item = "one")))
            val prior = cache.view.value
            val result = data.items.edit("one", 1, ItemEdit().text("title", "Renamed"), key = "rename")
            assertIs<WriteResult.Success<ItemResponse>>(result)
            assertTrue(result.refreshPending)
            assertEquals("Renamed", cache.view.value.items.single().title)
            assertEquals(prior.board, cache.view.value.board)
            assertEquals(prior.agenda, cache.view.value.agenda)
            assertEquals(1L, cache.sync.value?.cursor)
        } finally { api.close(); cache.close() }
    }
    @Test fun occurrenceWriteReplacesBoardAndAgendaTogetherInPlace() = runTest {
        val cache = memoryCache { now }
        var agendaFails = true
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/occurrences/occ" -> json(WireJson.encodeToString(occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2)))
                "/v1/items/one" -> json(WireJson.encodeToString(itemResponse("one")))
                "/v1/items/one/occurrences" -> json(WireJson.encodeToString(OccurrencePage(listOf(occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2)), null)))
                "/v1/members" -> json("""{"items":[],"next_cursor":null}""")
                "/v1/board" -> json(page(boardRow("one", "occ", "B", "doing")))
                else -> if (agendaFails) throw ConnectFailure() else json(page(agenda("occ", item = "one", due = LocalDate.parse("2026-10-09"))))
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            cache.replaceBoard(listOf(boardRow("one", "occ"))); cache.replaceAgenda(listOf(agenda("occ", item = "one")))
            val prior = cache.view.value
            val failed = data.occurrences.status("occ", 1, OccurrenceStatus.doing, "first")
            assertIs<WriteResult.Success<OccurrenceResponse>>(failed); assertTrue(failed.refreshPending)
            assertEquals("doing", cache.view.value.occurrences.single().status)
            assertEquals(prior.board, cache.view.value.board) // neither projection switched alone
            assertEquals(prior.agenda, cache.view.value.agenda)
            agendaFails = false
            val ok = data.occurrences.status("occ", 2, OccurrenceStatus.doing, "second")
            assertIs<WriteResult.Success<OccurrenceResponse>>(ok); assertFalse(ok.refreshPending)
            assertEquals(listOf("B"), cache.view.value.board.map { it.occurrence!!.boardPosition })
            assertEquals(listOf(LocalDate.parse("2026-10-09")), cache.view.value.agenda.map { it.agenda!!.dueDate })
        } finally { api.close(); cache.close() }
    }
    // N-1: a snapshot computed before an occurrence write must not leave the write's rows older after it installs.
    // The mock follows the server's window rule (items/read.rs): a from/to request never returns undated occurrences,
    // so an undated row (the typical board card) can only be repaired from what the client already holds.
    @Test fun occurrenceWriteDuringAnInFlightSnapshotSurvivesItsInstall() = runTest {
        data class Case(val name: String, val row: OccurrenceRead, val repairFails: Boolean = false)
        for (case in listOf(
            Case("undated", occurrenceRead("occ", "one")),
            Case("dated", occurrenceRead("occ", "one").copy(dueAt = now + 1.days)),
            Case("undated, repair refetch fails", occurrenceRead("occ", "one"), repairFails = true),
        )) {
            val cache = memoryCache { now }
            val agendaStarted = CompletableDeferred<Unit>(); val releaseAgenda = CompletableDeferred<Unit>()
            var agendaCalls = 0; var itemGets = 0
            // Server state after the write; the snapshot below was computed before it (todo v1).
            val written = occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2, dueAt = case.row.dueAt)
            val api = transport { req ->
                when (req.url.encodedPath) {
                    "/v1/snapshot" -> json(WireJson.encodeToString(emptySnapshot(2).copy(items = listOf(itemRead("one")), occurrences = listOf(case.row))))
                    "/v1/board" -> json(page(boardRow("one", "occ")))
                    "/v1/agenda" -> { if (++agendaCalls == 1) { agendaStarted.complete(Unit); releaseAgenda.await() }; json(page()) }
                    "/v1/occurrences/occ" -> json(WireJson.encodeToString(written))
                    "/v1/items/one" -> { if (++itemGets > 1 && case.repairFails) throw ConnectFailure(); json(WireJson.encodeToString(itemResponse("one"))) }
                    "/v1/items/one/occurrences" -> {
                        val windowed = req.url.parameters["from"] != null || req.url.parameters["to"] != null
                        val rows = listOf(written).filter { !windowed || req.url.parameters["undated"] == "true" || (it.startsAt ?: it.dueAt) != null }
                        json(WireJson.encodeToString(OccurrencePage(rows, null)))
                    }
                    else -> json("""{"items":[],"next_cursor":null}""")
                }
            }
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(case.row)))
                val refresh = async { runCatching { data.sync.refresh() } } // a failed child would cancel the test scope
                agendaStarted.await() // the refresh holds the older snapshot and is still paging projections
                val result = data.occurrences.status("occ", 1, OccurrenceStatus.doing, "write")
                assertIs<WriteResult.Success<OccurrenceResponse>>(result, case.name)
                assertEquals("doing", cache.view.value.occurrences.single().status, case.name)
                releaseAgenda.complete(Unit)
                val outcome = refresh.await()
                assertEquals(2L, cache.sync.value?.cursor, case.name) // the snapshot installed either way
                val row = cache.view.value.occurrences.single()
                assertEquals("doing" to 2, row.status to row.version, case.name)
                if (case.repairFails) {
                    assertIs<SyncUnavailable>(outcome.exceptionOrNull(), case.name)
                    assertEquals(SyncStatus.Failed("connection-failed"), data.sync.status.value, case.name)
                } else assertTrue(outcome.isSuccess, case.name)
            } finally { api.close(); cache.close() }
        }
    }
    // O-c: member/settings write responses that land while an older snapshot is in flight survive its install too.
    @Test fun memberAndSettingsWritesDuringAnInFlightSnapshotSurviveItsInstall() = runTest {
        val cache = memoryCache { now }
        val agendaStarted = CompletableDeferred<Unit>(); val releaseAgenda = CompletableDeferred<Unit>()
        var agendaCalls = 0
        val member = WireJson.decodeFromString<MemberRead>("""{"id":"self","household_id":"household","display_name":"Self","role":"member","color":"red","initials":"S","tzid":"UTC","tz_source":"manual","version":1,"pending_derivation":false}""")
        val older = emptySnapshot(2).copy(members = listOf(member))
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/snapshot" -> json(WireJson.encodeToString(older))
                "/v1/agenda" -> { if (++agendaCalls == 1) { agendaStarted.complete(Unit); releaseAgenda.await() }; json(page()) }
                "/v1/members/self" -> json(WireJson.encodeToString(member.copy(color = "blue", version = 2)))
                "/v1/me/settings" -> json(WireJson.encodeToString(older.memberSettings.copy(version = 2)))
                else -> json(page())
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(members = listOf(member)))
            val refresh = async { data.sync.refresh() }
            agendaStarted.await()
            assertIs<WriteResult.Success<MemberRead>>(data.household.patchMember("self", 1, MemberPatch(color = "blue")))
            assertIs<WriteResult.Success<SettingsRead>>(data.household.putSettings(1, older.memberSettings.settings))
            releaseAgenda.complete(Unit); refresh.await()
            assertEquals(2L, cache.sync.value?.cursor)
            assertEquals("blue" to 2, cache.view.value.members.single().let { it.color to it.version })
            assertEquals(2, cache.view.value.settings?.version)
        } finally { api.close(); cache.close() }
    }
    // M-1/M-2: which receipts the staged install re-applies, and that it does so in the same published view.
    @OptIn(ExperimentalCoroutinesApi::class)
    @Test fun receiptGuardsApplyOnlyNewerVersionsOfRowsTheSnapshotHolds() = runTest {
        val cache = memoryCache { now }
        fun occ(id: String, item: String, version: Int, status: String = "todo") = occurrenceRead(id, item).copy(version = version, status = status)
        fun wrote(id: String, item: String, version: Int) = occurrenceResponse(id, item).copy(status = OccurrenceStatus.doing, version = version)
        fun state() = cache.view.value.occurrences.associate { it.id to (it.status to it.version) }
        val snapshot = emptySnapshot(2).copy(items = listOf(itemRead("one")),
            occurrences = listOf(occ("newer", "one", 1), occ("older", "one", 3), occ("early", "one", 1)))
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"), itemRead("two")), occurrences = listOf(
                occ("newer", "one", 1), occ("older", "one", 3), occ("early", "one", 1), occ("hidden", "two", 1), occ("deleted", "one", 1))))
            cache.putOccurrence(wrote("early", "one", 2)) // received before snapshot request: never re-applied
            val mark = cache.receiptMark()
            cache.putOccurrence(wrote("newer", "one", 2))   // newer than the snapshot's v1: re-applied
            cache.putOccurrence(wrote("older", "one", 2))   // the snapshot already has v3: skipped
            cache.putOccurrence(wrote("hidden", "two", 2))  // item two is no longer visible: skipped
            cache.putOccurrence(wrote("deleted", "one", 2)) // the snapshot (which carries every undated row) lacks it: skipped
            val views = mutableListOf<CacheView>()
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { cache.view.collect { views += it } }
            cache.installStaged(snapshot, now, receivedAfter = mark)
            assertEquals(mapOf("newer" to ("doing" to 2), "older" to ("todo" to 3), "early" to ("todo" to 1)), state())
            // Atomic: no published view ever showed the snapshot's older row of the re-applied write.
            assertTrue(views.none { view -> view.occurrences.any { it.id == "newer" && it.version == 1 } })
            // The install consumed receipts; repeating the snapshot keeps its own rows.
            cache.installStaged(snapshot, now, receivedAfter = mark)
            assertEquals("todo" to 1, state()["newer"])
            // clear() drops receipts too.
            cache.putOccurrence(wrote("newer", "one", 9)); cache.clear()
            cache.installStaged(snapshot, now, receivedAfter = 0)
            assertEquals("todo" to 1, state()["newer"])
        } finally { cache.close() }
    }
    // O-g: a delete that commits while an older snapshot is in flight must not be undone by its install.
    @Test fun itemDeleteDuringAnInFlightSnapshotIsNotUndone() = runTest {
        val cache = memoryCache { now }
        val agendaStarted = CompletableDeferred<Unit>(); val releaseAgenda = CompletableDeferred<Unit>()
        var agendaCalls = 0
        val before = emptySnapshot(2).copy(items = listOf(itemRead("keep"), itemRead("gone")), occurrences = listOf(occurrenceRead("occ", "gone")))
        val api = transport { req ->
            when (req.url.encodedPath) {
                "/v1/snapshot" -> json(WireJson.encodeToString(before)) // computed before the delete
                "/v1/board" -> json(page(boardRow("gone", "occ")))
                "/v1/agenda" -> { if (++agendaCalls == 1) { agendaStarted.complete(Unit); releaseAgenda.await() }; json(page()) }
                "/v1/items/gone" -> json(WireJson.encodeToString(itemResponse("gone").copy(version = 2, deletedAt = now)))
                else -> json(page())
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("keep"), itemRead("gone")), occurrences = listOf(occurrenceRead("occ", "gone"))))
            val refresh = async { data.sync.refresh() }
            agendaStarted.await()
            val deleted = data.items.delete("gone", 1, "delete")
            assertIs<WriteResult.Success<ItemResponse>>(deleted); assertFalse(deleted.refreshPending)
            assertEquals(listOf("keep"), cache.view.value.items.map { it.id })
            releaseAgenda.complete(Unit); refresh.await()
            assertEquals(2L, cache.sync.value?.cursor)
            assertEquals(listOf("keep"), cache.view.value.items.map { it.id })
            assertTrue(cache.view.value.occurrences.isEmpty()); assertTrue(cache.view.value.board.isEmpty())
        } finally { api.close(); cache.close() }
    }
    @Test fun agendaRefreshReplacesOnlyTheFetchedMemberWindow() = runTest {
        val cache = memoryCache { now }
        var own = listOf(agenda("mine", item = "one", due = LocalDate.parse("2026-10-07")))
        val api = transport { req ->
            if (req.url.parameters["member_id"] == "other") json(page(agenda("theirs", item = "two", member = "other", due = LocalDate.parse("2026-10-08"))))
            else json(page(*own.toTypedArray()))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"), itemRead("two"))))
            data.agenda.refresh(now - 30.days, now + 30.days)
            data.agenda.refresh(now - 30.days, now + 30.days, memberId = "other")
            assertEquals(setOf("mine", "theirs"), cache.view.value.agenda.map { it.agenda!!.occurrenceId }.toSet())
            own = emptyList() // the own row is provably gone from a window that covers it
            data.agenda.refresh(now - 30.days, now + 30.days)
            assertEquals(listOf("theirs"), cache.view.value.agenda.map { it.agenda!!.occurrenceId })
        } finally { api.close(); cache.close() }
    }
    @Test fun undatedTodoStatusWriteSurvivesProductionWindowRefetch() = runTest {
        val cache = memoryCache { now }
        val fetched = mutableListOf<String>()
        val api = transport { req ->
            fetched += req.url.encodedPath
            when (req.url.encodedPath) {
                "/v1/occurrences/occ" -> json(WireJson.encodeToString(occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2)))
                "/v1/items/one" -> json(WireJson.encodeToString(itemResponse("one")))
                // C4 extension: window + undated=true returns the undated authoritative row.
                "/v1/items/one/occurrences" -> {
                    assertEquals("true", req.url.parameters["undated"])
                    json(WireJson.encodeToString(OccurrencePage(listOf(occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2)), null)))
                }
                "/v1/members" -> json("""{"items":[],"next_cursor":null}""")
                else -> json(page())
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            val result = data.occurrences.status("occ", 1, OccurrenceStatus.doing, "status")
            assertIs<WriteResult.Success<OccurrenceResponse>>(result)
            assertFalse(result.refreshPending)
            assertTrue("/v1/items/one/occurrences" in fetched)
            assertEquals("doing", cache.view.value.occurrences.single().status)
            assertEquals(2, cache.view.value.occurrences.single().version)
        } finally { api.close(); cache.close() }
    }
    @Test fun anotherDevicesUndatedTickleRefetchesWithoutAWriteReceipt() = runTest {
        val cache = memoryCache { now }
        val api = transport { req -> when (req.url.encodedPath) {
            "/v1/items/one" -> json(WireJson.encodeToString(itemResponse("one")))
            "/v1/items/one/occurrences" -> {
                assertEquals("true", req.url.parameters["undated"])
                assertNotNull(req.url.parameters["from"]); assertNotNull(req.url.parameters["to"])
                json(WireJson.encodeToString(OccurrencePage(listOf(occurrenceResponse("occ", "one").copy(status = OccurrenceStatus.doing, version = 2)), null)))
            }
            else -> json(page())
        } }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            data.sync.refetchByIds(occurrenceIds = setOf("occ"))
            assertEquals("doing" to 2, cache.view.value.occurrences.single().let { it.status to it.version })
        } finally { api.close(); cache.close() }
    }
    @Test fun windowedOccurrenceRefreshReplacesOnlyProvablyCoveredRows() = runTest {
        val cache = memoryCache { now }
        val updated = occurrenceResponse("updated", "one").copy(startsAt = now + 2.days, endsAt = now + 2.days + 1.hours, status = OccurrenceStatus.doing)
        val api = transport { json(WireJson.encodeToString(OccurrencePage(listOf(updated), null))) }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"), itemRead("two")), occurrences = listOf(
                occurrenceRead("undated", "one"),
                occurrenceRead("gone", "one").copy(startsAt = now + 1.days, endsAt = now + 1.days + 1.hours),
                occurrenceRead("gone-date", "one").copy(dueDate = LocalDate.parse("2026-10-11")),
                occurrenceRead("updated", "one").copy(startsAt = now + 2.days, endsAt = now + 2.days + 1.hours),
                occurrenceRead("beyond", "one").copy(dueAt = now + 400.days),
                occurrenceRead("other-item", "two").copy(dueAt = now + 3.days),
            )))
            data.occurrences.refresh("one", now - 30.days, now + 30.days)
            assertEquals(setOf("undated", "updated", "beyond", "other-item"), cache.view.value.occurrences.map { it.id }.toSet())
            assertEquals("doing", cache.view.value.occurrences.single { it.id == "updated" }.status)
        } finally { api.close(); cache.close() }
    }
}
fun agenda(id: String, item: String = "item-$id", member: String = "self", due: LocalDate? = null): ItemRead =
    itemRead(item).copy(agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"household","item_id":"$item","derived_at":"2026-10-06T12:00:00Z","member_id":"$member","occurrence_id":"$id","due_date":${due?.let { "\"$it\"" } ?: "null"}}"""))
