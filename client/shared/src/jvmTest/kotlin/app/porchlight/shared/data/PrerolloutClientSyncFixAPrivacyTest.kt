package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.screens.calendar.calendarConflictLabel
import io.ktor.http.Url
import kotlinx.datetime.TimeZone
import kotlinx.serialization.json.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Duration.Companion.days

class PrerolloutClientSyncFixAPrivacyTest {
    private val f = SyncBundleFixtures

    @Test fun restoredReferenceStillRejectsDecorationsFromThePreEvictionGeneration() = runTest {
        val cache = memoryCache { f.now }
        val private = f.snapshot.items.single().copy(id = "private-peer", title = "Revoked appointment")
        val snapshot = f.snapshot.copy(items = f.snapshot.items + private)
        val conflict = ConflictRead(f.snapshot.memberSettings.memberId, f.agenda.agenda!!.occurrenceId,
            private.title, f.now, f.now + 1.days, "hard", otherItemId = private.id)
        val page = f.bundle(agenda = listOf(f.agenda.copy(conflicts = listOf(conflict))))
        try {
            cache.install(snapshot)
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val late = async { cache.inSession { fence ->
                started.complete(Unit); release.await()
                cache.installItemBundle(fence, ItemBundleCoverage(listOf(page.coverage)), ItemBundleRows(listOf(page)))
            } }
            started.await(); cache.evictItem(private.id); cache.install(snapshot)
            assertTrue(cache.view.value.items.any { it.id == private.id })
            release.complete(Unit); late.await()
            assertEquals(listOf(conflict.copy(otherItemId = null, otherOccurrenceId = null, otherTitle = "Busy")),
                cache.view.value.agenda.flatMap { it.conflicts.orEmpty() })
            cache.installBundle(page)
            assertEquals(listOf(conflict), cache.view.value.agenda.single().conflicts, "a genuinely newer authorized reply can restore decorations")
        } finally { cache.close() }
    }

    @Test fun evictionRedactsPeerConflictsAndHeldBundleSnapshotAndProjectionReplies() = runTest {
        for (eviction in listOf("targeted", "full", "legacy")) for (reply in listOf("bundle", "snapshot", "projection")) {
            val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
            PorchlightDb.Schema.create(driver)
            val sql = PorchlightDb(driver).cacheQueries
            val cache = ReadCache(driver) { f.now }
            val private = f.snapshot.items.single().copy(id = "private-item", title = "Revoked appointment")
            val privateOccurrence = f.snapshot.occurrences.single().copy(id = "private-occurrence", itemId = private.id)
            val conflict = ConflictRead(f.snapshot.memberSettings.memberId, f.agenda.agenda!!.occurrenceId,
                private.title, f.now, f.now + 1.days, "hard", otherItemId = private.id, otherOccurrenceId = privateOccurrence.id)
            val peer = f.agenda.copy(conflicts = listOf(conflict, conflict.copy(kind = "load")))
            val inherited = f.recording<ListResponse>("list-occurrence-template").copy(itemId = private.id, name = private.title)
            val snapshot = f.snapshot.copy(items = f.snapshot.items + private, occurrences = f.snapshot.occurrences + privateOccurrence, lists = f.snapshot.lists + inherited,
                notifications = f.snapshot.notifications.map { it.copy(itemId = null, summary = it.summary.copy(title = private.title, body = private.title, text = private.title)) })
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (Url(testOrigin() + path).encodedPath) {
                    "/v1/snapshot" -> WireJson.encodeToJsonElement(f.snapshot).jsonObject
                    "/v1/board" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                    "/v1/agenda" -> buildJsonObject { put("items", JsonArray(listOf(WireJson.encodeToJsonElement(peer)))) }
                    else -> error("Later peer and inbox reads unavailable")
                }
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }), { f.now })
            data.itemSyncCapability = { eviction != "legacy" }; data.targetedSyncVerified = true
            fun assertRedacted() {
                assertTrue(cache.view.value.agenda.flatMap { it.conflicts.orEmpty() }.none { it.otherTitle == private.title || it.otherItemId == private.id || it.otherOccurrenceId == privateOccurrence.id }, "$eviction/$reply published warnings")
                assertTrue(sql.allAgendas().executeAsList().none { private.title in it.payload || private.id in it.payload || privateOccurrence.id in it.payload }, "$eviction/$reply SQL")
                assertTrue(cache.view.value.agenda.flatMap { it.conflicts.orEmpty() }.map { calendarConflictLabel(it, TimeZone.UTC) }.none { private.title in it }, "rendered calendar warning labels")
                assertTrue(cache.view.value.lists.none { it.itemId == private.id || it.name == private.title }, "inherited lists")
                assertTrue(cache.view.value.notifications.none { private.title in it.summary.toString() }, "inbox summaries")
            }
            try {
                cache.install(snapshot, projections = Projections(emptyList(), AgendaWindow(f.from, f.to, null, listOf(peer))))
                val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
                val late = async {
                    cache.inSession { fence ->
                        started.complete(Unit); release.await()
                        when (reply) {
                            "bundle" -> { val page = f.bundle(agenda = listOf(peer)); cache.installItemBundle(fence, ItemBundleCoverage(listOf(page.coverage)), ItemBundleRows(listOf(page))) }
                            "snapshot" -> cache.install(fence, snapshot, projections = Projections(emptyList(), AgendaWindow(f.from, f.to, null, listOf(peer))))
                            else -> cache.replaceAgenda(fence, AgendaWindow(f.from, f.to, null, listOf(peer)))
                        }
                    }
                }
                started.await()
                if (eviction == "full") data.sync.refresh()
                else {
                    val event = StreamFrame.Tickle(100, "item.access_changed", "item", private.id, evicted = true)
                    if (eviction == "targeted") assertFailsWith<IllegalStateException> { data.applyStreamFrame(event) }
                    else data.applyStreamFrame(event)
                    assertTrue(cache.view.value.notifications.isEmpty(), "immediate summary invalidation")
                }
                assertRedacted()
                assertFailsWith<IllegalStateException> { data.items.get(peer.id) }
                release.complete(Unit); late.await(); assertRedacted()
            } finally { data.close() }
        }
    }
}
