package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.data.syncmodel.HouseholdModel
import app.porchlight.shared.data.syncmodel.HouseholdModel.Kind
import app.porchlight.shared.data.syncmodel.HouseholdModel.Mutation
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import io.ktor.http.Url
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import java.io.IOException
import kotlin.random.Random
import kotlin.test.*
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncDifferentialTest {
    private class Device(val model: HouseholdModel, val scope: TestScope, var reader: String = model.readers[0], val targeted: Boolean = true) {
        // A keeper connection lets a genuinely new driver/cache/controller reopen the same SQLite database.
        private val url = "jdbc:sqlite:file:sync-proof-${nextDatabase++}?mode=memory&cache=shared"
        private val keeper = JdbcSqliteDriver(url).also { PorchlightDb.Schema.create(it) }
        private lateinit var driver: JdbcSqliteDriver
        lateinit var cache: ReadCache
        lateinit var data: HouseholdData
        private var identity = 1L
        var requests = 0
        var bytes = 0L
        val paths = mutableListOf<String>()
        var failure: ((Url) -> Exception?)? = null
        var afterRead: suspend (Url) -> Unit = {}
        private val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("GET", method)
                val url = Url(testOrigin() + path)
                requests++; paths += url.encodedPath
                failure?.invoke(url)?.let { throw it }
                val response = model.response(reader, url)
                bytes += response.toString().toByteArray(Charsets.UTF_8).size
                afterRead(url)
                return response
            }
        }
        init { open() }
        private fun open() {
            driver = JdbcSqliteDriver(url)
            cache = ReadCache(driver) { HouseholdModel.now }
            cache.sessionEpoch = { identity }
            data = HouseholdData(cache, GeneratedCall({ transport }, { "model-session-$reader" }, jsonDispatcher = StandardTestDispatcher(scope.testScheduler)),
                { HouseholdModel.now }, syncDispatcher = StandardTestDispatcher(scope.testScheduler))
            data.itemSyncCapability = { targeted }
            data.targetedSyncVerified = true
            data.sync.waitForReadRetry = { delay(it) }
        }
        suspend fun full() {
            data.sync.resnapshot()
            for (member in model.readers) data.agenda.refresh(HouseholdModel.from, HouseholdModel.to, member)
            if (cache.view.value.lists.any { it.id == HouseholdModel.id(501) } && cache.view.value.occurrences.any { it.id == HouseholdModel.id(250) })
                data.lists.get(HouseholdModel.id(501), HouseholdModel.id(250))
        }
        suspend fun restart() {
            data.close(); open(); cache.initialize()
            // No refresh: durable retained scopes must survive process loss.
        }
        suspend fun swap() {
            identity++
            data.sync.clear()
            reader = model.readers.single { it != reader }
            full()
        }
        suspend fun deliver(frames: List<StreamFrame>, duplicate: Boolean = false) {
            // Replay is ordered within each connection; older duplicate frames may follow an acknowledged reconnect.
            val negotiated = if (targeted) frames else frames.filterNot { it is StreamFrame.ProjectionChanged || it is StreamFrame.Tickle &&
                (it.kind.endsWith(".invalidated") || it.kind == "settings.changed" && it.entityType == "availability") }
            val delivered = if (duplicate) negotiated + negotiated else negotiated
            val source = StreamSource { _, receive ->
                receive(StreamFrame.Ready)
                delivered.forEach { delay(1); receive(it) }
            }
            val live = LiveSync(source, { cache.sync.value?.cursor }, { data.applyStreamFrame(it) }, { cache.acknowledgeCursor(it) }, { data.sync.resnapshot() },
                { HouseholdModel.now }, concurrentApply = { targeted })
            live.connectOnce()
        }
        fun tableRows(): Map<String, List<String>> {
            val tables = listOf("CacheMember", "CacheItem", "CacheOccurrence", "CacheAgenda", "CacheBoard", "CacheSettings", "CacheReminder",
                "CacheWrite", "CacheNotification", "CacheNotificationPrefs", "CachePlanning", "CacheProjectionScope", "CacheList", "CacheEntry", "CacheComment", "CacheAttachment")
            val columns = mapOf("CacheAgenda" to 5, "CacheReminder" to 5, "CacheProjectionScope" to 3, "CacheList" to 5, "CacheEntry" to 5,
                "CacheCommentHistory" to 5, "Staging" to 6)
            return (tables + listOf("CacheCommentHistory", "CacheOwner", "Staging")).associateWith { table ->
                driver.executeQuery(null, "SELECT * FROM $table", { cursor ->
                    val rows = mutableListOf<String>()
                    while (cursor.next().value) rows += JsonArray((0 until (columns[table] ?: 4)).map { index ->
                        cursor.getString(index)?.let(::JsonPrimitive) ?: JsonNull
                    }).toString()
                    app.cash.sqldelight.db.QueryResult.Value(rows.sorted())
                }, 0).value
            }
        }
        private inline fun <reified T> encoded(rows: List<T>) = rows.map { WireJson.encodeToJsonElement(it).toString() }.sorted()
        fun presentationRows(): Map<String, List<String>> = cache.view.value.let { v -> mapOf(
            "members" to encoded(v.members), "items" to encoded(v.items), "occurrences" to encoded(v.occurrences),
            "board" to encoded(v.board), "agenda" to encoded(v.agenda), "reminders" to encoded(v.reminders),
            "notifications" to encoded(v.notifications), "prefs" to encoded(listOfNotNull(v.notificationPrefs)),
            "availability" to encoded(v.availability), "suggestions" to encoded(v.suggestions), "feeds" to encoded(v.sources),
            "lists" to encoded(v.lists), "comments" to encoded(v.comments), "attachments" to encoded(v.attachments),
            "settings" to encoded(listOfNotNull(v.settings)), "writes" to encoded(v.itemWrites.values.toList())
        ) }
        fun resetCounts() { requests = 0; bytes = 0; paths.clear() }
        fun close() { data.close(); keeper.close() }
        companion object { var nextDatabase = 0 }
    }

    private suspend fun equivalent(target: Device, oracle: Device, label: String) {
        // A new full baseline, never refreshed using the targeted replies under comparison.
        oracle.data.sync.clear()
        oracle.reader = target.reader
        oracle.full()
        if (target.cache.view.value.boardScope != "household") oracle.data.board.refresh(target.cache.view.value.boardScope)
        for (window in target.cache.view.value.agendaWindows - oracle.cache.view.value.agendaWindows) {
            var from = window.from
            while (from < window.to) {
                val to = minOf(from + 730.days, window.to)
                oracle.data.agenda.refresh(from, to, window.memberId)
                from = to
            }
        }
        val expected = oracle.tableRows()
        val actual = target.tableRows()
        for (table in expected.keys) {
            val missing = expected.getValue(table) - actual.getValue(table).toSet()
            val extra = actual.getValue(table) - expected.getValue(table).toSet()
            assertTrue(missing.isEmpty() && extra.isEmpty(), "$label table=$table missing=$missing extra=$extra")
        }
        for ((kind, rows) in oracle.presentationRows()) assertEquals(rows, target.presentationRows()[kind], "$label presentation=$kind")
        assertEquals(oracle.cache.view.value.commentHistory, target.cache.view.value.commentHistory, "$label comment histories")
        assertEquals(oracle.cache.view.value.agendaWindows, target.cache.view.value.agendaWindows, "$label retained agenda scopes")
        val unreadable = target.model.items.filterNot { target.model.visible(it, target.reader) }.mapTo(mutableSetOf()) { it.id }
        assertTrue(target.cache.view.value.items.none { it.id in unreadable }, "$label evicted item reappeared")
        assertTrue(target.cache.view.value.comments.none { it.itemId in unreadable }, "$label evicted child reappeared")
    }

    private suspend fun TestScope.campaign(seed: Int, mutations: List<Mutation>) {
        val model = HouseholdModel(longThread = seed % 20 == 0)
        val target = Device(model, this)
        val oracle = Device(model, this, targeted = false)
        try {
            target.full()
            if (seed % 2 == 1) target.data.board.refresh("mine")
            equivalent(target, oracle, "seed=$seed initial")
            for ((index, mutation) in mutations.withIndex()) {
                val frames = model.mutate(mutation).getValue(target.reader)
                // A transient fault is replayed through a new real LiveSync connection, never installed as partial coverage.
                val fault = index == 2 && seed % 4 != 0
                var failed = false
                if (fault) target.failure = { url ->
                    if (!failed && (url.encodedPath.endsWith("/sync") || url.encodedPath.endsWith("/comments") || url.encodedPath == "/v1/notifications")) {
                        failed = true
                        when (seed % 4) {
                            1 -> ApiProblem("rate-limited", 429, "Retry", retryAfterMillis = 1200)
                            2 -> ApiProblem("unavailable", 503, "Retry", retryAfterMillis = 1700)
                            else -> IOException("Injected transport failure")
                        }
                    } else null
                }
                var attempts = 0
                while (true) {
                    try { target.deliver(frames, duplicate = seed % 3 == 0); break }
                    catch (e: CancellationException) { throw e }
                    catch (e: Exception) {
                        if (e !is StaleVisibilityReply && !failed || ++attempts >= 8) throw e
                        target.failure = null
                        // Reconnect after failed or privacy-fenced reads; prior application-owned cohorts may still drain.
                        delay(2000)
                    }
                }
                target.failure = null
                equivalent(target, oracle, "seed=$seed step=$index mutation=$mutation")
                if (index == 3 && seed % 5 == 0) {
                    target.restart()
                    equivalent(target, oracle, "seed=$seed restart")
                }
                if (index == 4 && seed % 7 == 0) {
                    target.swap()
                    equivalent(target, oracle, "seed=$seed identity swap")
                }
            }
        } finally { target.close(); oracle.close() }
    }

    @Test fun seededHouseholdMutationsEqualFreshFullRefresh() = runTest(timeout = 180_000.milliseconds) {
        val count = System.getenv("PORCH_SYNC_CLIENT_SEEDS")?.toInt()?.coerceAtLeast(200) ?: 200
        val only = System.getenv("PORCH_SYNC_CLIENT_SEED")?.toInt()
        val seeds = if (only == null) 0 until count else only..only
        val start = System.nanoTime()
        for (seed in seeds) {
            val random = Random(seed)
            val mutations = List(8) { Mutation(Kind.entries[random.nextInt(Kind.entries.size)], random.nextInt(2), random.nextInt(6)) }
            try { campaign(seed, mutations) }
            catch (failure: Throwable) {
                // Deletion shrink: rerun candidates from a clean household and DB, retaining only failing subsequences.
                var shrunk = mutations
                var index = 0
                while (index < shrunk.size && shrunk.size > 1) {
                    val candidate = shrunk.filterIndexed { i, _ -> i != index }
                    val stillFails = try { campaign(seed, candidate); false }
                    catch (e: CancellationException) { throw e }
                    catch (e: Throwable) { e::class == failure::class }
                    if (stillFails) shrunk = candidate else index++
                }
                throw AssertionError("Differential seed=$seed shrunk mutations=$shrunk; original=${failure.message}", failure)
            }
        }
        println("client-sync differential seeds=${seeds.count()} elapsed_ms=${(System.nanoTime() - start) / 1_000_000}")
    }

    @Test fun reparentedReminderIsNotDeletedByFormerParentReplacement() = runTest {
        campaign(1, listOf(Mutation(Kind.REPARENT, 0, 3)))
    }

    @Test fun itemEvictionRemovesInheritedOccurrenceChecks() = runTest {
        campaign(2, listOf(Mutation(Kind.ACL, 0, 5)))
    }

    @Test fun restartRetainsOtherMemberAgendaCoverageForSubsequentEdits() = runTest {
        val model = HouseholdModel()
        val target = Device(model, this)
        val oracle = Device(model, this, targeted = false)
        try {
            target.full(); target.restart()
            target.deliver(model.mutate(Mutation(Kind.TITLE, 0, 0)).getValue(target.reader))
            equivalent(target, oracle, "restart then title")
        } finally { target.close(); oracle.close() }
    }

    @Test fun evictionDuringBundleFetchCannotRestoreItemOrInheritedChecks() = runTest {
        val model = HouseholdModel(longThread = true)
        val target = Device(model, this)
        val oracle = Device(model, this, targeted = false)
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        try {
            target.full()
            target.afterRead = { url -> if (url.encodedPath.endsWith("/sync")) { started.complete(Unit); release.await() } }
            val edit = model.mutate(Mutation(Kind.TITLE, 0, 5)).getValue(target.reader)
            val pending = async { runCatching { target.deliver(edit) } }
            started.await()
            val eviction = model.mutate(Mutation(Kind.ACL, 0, 5)).getValue(target.reader)
            val evicted = eviction.filterIsInstance<StreamFrame.Tickle>().single { it.evicted }
            target.data.applyStreamFrame(evicted)
            assertTrue(target.cache.view.value.items.none { it.id == evicted.entityId })
            release.complete(Unit); pending.await()
            target.afterRead = {}
            target.deliver(eviction)
            equivalent(target, oracle, "eviction during bundle fetch")
        } finally { release.complete(Unit); target.close(); oracle.close() }
    }

    @Test fun failedCommentContinuationInstallsNothingAndReplayRepairsIt() = runTest {
        for (status in listOf(429, 503, 0)) {
            val model = HouseholdModel(longThread = true)
            val target = Device(model, this)
            val oracle = Device(model, this, targeted = false)
            try {
                target.full()
                val before = target.tableRows()
                val oldCursor = target.cache.sync.value!!.cursor
                val dependentFrames = model.mutate(Mutation(Kind.COMMENT, 1, 0)).getValue(target.reader)
                val item = HouseholdModel.id(100)
                val frame = StreamFrame.Tickle(model.cursor + 2, "item.invalidated", "item", item, false)
                var failed = false
                target.failure = { url -> if (url.parameters["comments_cursor"] != null) {
                    failed = true
                    if (status == 0) IOException("Injected page failure") else ApiProblem("retry", status, "Retry", retryAfterMillis = 1800)
                } else null }
                assertFailsWith<Exception> { target.deliver(listOf(frame)) }
                assertTrue(failed, "Continuation fault was reached")
                assertEquals(before, target.tableRows(), "No partial bundle may commit")
                assertEquals(oldCursor, target.cache.sync.value?.cursor)
                target.failure = null
                target.deliver(listOf(frame))
                // The source mutation also released inbox work, independently of the failing bundle.
                dependentFrames.filterIsInstance<StreamFrame.Tickle>().filter { it.entityType == "notification" }.forEach { target.data.applyStreamFrame(it) }
                equivalent(target, oracle, "continuation retry status=$status")
            } finally { target.close(); oracle.close() }
        }
    }

    @Test fun restartDuringFetchReplaysFromDurableUnacknowledgedCursor() = runTest {
        val model = HouseholdModel(longThread = true)
        val target = Device(model, this); val oracle = Device(model, this, targeted = false)
        val started = CompletableDeferred<Unit>()
        try {
            target.full()
            val cursor = target.cache.sync.value!!.cursor
            val frames = model.mutate(Mutation(Kind.TITLE, 0, 0)).getValue(target.reader)
            target.afterRead = { if (it.encodedPath.endsWith("/sync")) { started.complete(Unit); awaitCancellation() } }
            val interrupted = async { runCatching { target.deliver(frames) } }
            started.await(); target.restart()
            assertTrue(interrupted.await().isFailure)
            assertEquals(cursor, target.cache.sync.value?.cursor)
            target.afterRead = {}
            target.deliver(frames)
            equivalent(target, oracle, "restart during fetch")
        } finally { target.close(); oracle.close() }
    }

    @Test fun identityChangeDuringFetchRejectsOldHouseholdPayloads() = runTest {
        val model = HouseholdModel()
        val target = Device(model, this); val oracle = Device(model, this, targeted = false)
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        try {
            target.full()
            target.afterRead = { if (it.encodedPath.endsWith("/sync") && !started.isCompleted) { started.complete(Unit); release.await() } }
            val frames = model.mutate(Mutation(Kind.TITLE, 0, 0)).getValue(target.reader)
            val old = async { runCatching { target.deliver(frames) } }
            started.await()
            val swap = async { target.swap() }
            target.cache.view.first { it.sync == null }
            release.complete(Unit)
            old.await(); swap.await()
            equivalent(target, oracle, "identity changed during fetch")
        } finally { release.complete(Unit); target.close(); oracle.close() }
    }

    @Test fun projectionOverflowResnapshotReplacesAllRowsAndRetainedScopes() = runTest {
        val model = HouseholdModel(itemCount = 210)
        val target = Device(model, this); val oracle = Device(model, this, targeted = false)
        try {
            target.full()
            val frames = model.mutate(Mutation(Kind.RESET, 0, 0)).getValue(target.reader)
            assertIs<StreamFrame.ProjectionChanged>(frames.single())
            target.deliver(frames)
            equivalent(target, oracle, "projection invalidation overflow")
        } finally { target.close(); oracle.close() }
    }

    @Test fun duplicateEvictionDeliveryDoesNotLivelockInboxReconciliation() = runTest {
        campaign(177, listOf(Mutation(Kind.NOTICE_DELETE, 0, 5), Mutation(Kind.PURGE, 0, 0)))
    }

    @Test fun overlappingRetainedWindowsAreSplitAtTheWireLimit() = runTest {
        val model = HouseholdModel()
        val target = Device(model, this); val oracle = Device(model, this, targeted = false)
        try {
            target.full()
            target.data.agenda.refresh(HouseholdModel.from - 100.days, HouseholdModel.to - 100.days, target.reader)
            assertTrue(target.cache.view.value.agendaWindows.any { it.to - it.from > 730.days })
            target.deliver(model.mutate(Mutation(Kind.TITLE, 0, 0)).getValue(target.reader))
            equivalent(target, oracle, "overlapping retained agenda windows")
            target.restart()
            target.data.sync.resnapshot()
            equivalent(target, oracle, "resnapshot with long durable coverage")
        } finally { target.close(); oracle.close() }
    }

    @Test fun routineEditCostsAcrossFifteenDevices() = runTest {
        for (kind in listOf(Kind.TITLE, Kind.COMPLETE, Kind.COMMENT)) {
            val totals = mutableListOf<String>()
            for (targeted in listOf(true, false)) {
                val model = HouseholdModel(longThread = true)
                val devices = List(15) { Device(model, this, model.readers[it % 2], targeted) }
                try {
                    devices.forEach { it.full(); it.resetCounts() }
                    val frames = model.mutate(Mutation(kind, 0, 0))
                    devices.forEach { it.deliver(frames.getValue(it.reader)) }
                    totals += "${if (targeted) "targeted" else "legacy"}: per_device_requests=${devices.map { it.requests }} per_device_decoded_bytes=${devices.map { it.bytes }} total_requests=${devices.sumOf { it.requests }} total_bytes=${devices.sumOf { it.bytes }}"
                } finally { devices.forEach { it.close() } }
            }
            println("client-sync cost $kind devices=15 ${totals.joinToString("; ")}")
        }
    }
}
