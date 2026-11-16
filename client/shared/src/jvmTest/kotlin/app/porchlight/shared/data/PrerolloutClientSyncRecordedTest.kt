package app.porchlight.shared.data

import app.cash.sqldelight.db.QueryResult
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import java.util.zip.GZIPInputStream
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.milliseconds

/** Responses and invalidation closure come exclusively from real server handlers, never HouseholdModel. */
@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncRecordedTest {
    private fun recording(seed: Int): JsonObject = GZIPInputStream(checkNotNull(javaClass.getResourceAsStream("/sync-recordings/$seed.json.gz"))).use {
        WireJson.parseToJsonElement(it.reader().readText()).jsonObject
    }
    private fun requestKey(path: String): String {
        val url = Url("https://example.invalid$path")
        val parts = url.parameters.entries().flatMap { (k, values) -> values.map { "${k.encodeURLParameter()}=${it.encodeURLParameter()}" } }.sorted()
        return url.encodedPath + if (parts.isEmpty()) "" else "?${parts.joinToString("&")}"
    }
    private class Device(val scope: TestScope, var step: JsonObject, val member: Int) {
        private val url = "jdbc:sqlite:file:recorded-proof-${nextDatabase++}?mode=memory&cache=shared"
        private val keeper = JdbcSqliteDriver(url).also { PorchlightDb.Schema.create(it) }
        private lateinit var driver: JdbcSqliteDriver
        lateinit var cache: ReadCache
        lateinit var data: HouseholdData
        var faultStatus: Int? = null
        var faulted = false
        var delayed = false
        var requests = 0
        private val view get() = step.getValue("members").jsonArray[member].jsonObject
        private val now get() = Instant.parse(step.getValue("now").jsonPrimitive.content)
        private val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("GET", method)
                requests++
                val key = PrerolloutClientSyncRecordedTest().requestKey(path)
                val response = view.getValue("responses").jsonObject[key]?.jsonObject
                    ?: throw AssertionError("Missing recorded response member=$member operation=${step["operation"]} request=$key")
                if (delayed) delay(300)
                if (faultStatus != null && !faulted && path.contains("/sync?")) {
                    faulted = true
                    throw ApiProblem("retry", faultStatus!!, "Recorded request fault", retryAfterMillis = 1800)
                }
                val status = response.getValue("status").jsonPrimitive.int
                val answer = response.getValue("body").jsonObject
                if (status != 200) throw ApiProblem(answer.getValue("type").jsonPrimitive.content.substringAfterLast(':'), status, "Recorded server problem")
                return answer
            }
        }
        init { open() }
        private fun open() {
            driver = JdbcSqliteDriver(url)
            cache = ReadCache(driver) { now }
            data = HouseholdData(cache, GeneratedCall({ transport }, { "recorded-session" }, jsonDispatcher = StandardTestDispatcher(scope.testScheduler)),
                { now }, syncDispatcher = StandardTestDispatcher(scope.testScheduler))
            data.itemSyncCapability = { true }
            data.targetedSyncVerified = true
            data.sync.waitForReadRetry = { delay(it) }
        }
        suspend fun full() {
            data.sync.resnapshot()
            val windows = WireJson.decodeFromJsonElement<List<ItemAgendaWindow>>(step.getValue("windows"))
            for (window in windows) data.agenda.refresh(window.from, window.to, window.memberId)
        }
        suspend fun restart() { data.close(); open(); cache.initialize() }
        suspend fun deliver(duplicate: Boolean = false) {
            val wire = view.getValue("sse").jsonPrimitive.content
            val source = StreamSource { _, receive ->
                val parser = SseParser()
                wire.lineSequence().forEach { parser.line(it)?.let { frame ->
                    delay(1)
                    receive(frame)
                    // A second transcript after caught_up is unreachable: connectOnce
                    // ends at that barrier. Duplicate delivered positions before it.
                    if (duplicate && frame is StreamFrame.Tickle) receive(frame)
                } }
            }
            LiveSync(source, { cache.sync.value?.cursor }, { data.applyStreamFrame(it) }, { cache.acknowledgeCursor(it) }, { data.sync.resnapshot() }, { now },
                concurrentApply = { true }).connectOnce()
        }
        // Operational acknowledgement/tombstone generations intentionally differ from a fresh install.
        // Every content table, retained projection scope, scoped list row and staging row is compared.
        fun tables(): Map<String, List<String>> {
            val tables = listOf("CacheMember", "CacheItem", "CacheOccurrence", "CacheAgenda", "CacheBoard", "CacheSettings", "CacheReminder", "CacheWrite", "CacheNotification",
                "CacheNotificationPrefs", "CachePlanning", "CacheProjectionScope", "CacheList", "CacheEntry", "CacheComment", "CacheCommentHistory", "CacheAttachment", "CacheOwner", "Staging")
            return tables.associateWith { table ->
                val columns = driver.executeQuery(null, "PRAGMA table_info($table)", { c ->
                    var count = 0; while (c.next().value) count++
                    QueryResult.Value(count)
                }, 0).value
                driver.executeQuery(null, "SELECT * FROM $table", { c ->
                    val rows = mutableListOf<String>()
                    while (c.next().value) rows += JsonArray((0 until columns).map { c.getString(it)?.let(::JsonPrimitive) ?: JsonNull }).toString()
                    QueryResult.Value(rows.sorted())
                }, 0).value
            }
        }
        private inline fun <reified T> encoded(rows: List<T>) = rows.map { WireJson.encodeToJsonElement(it).toString() }.sorted()
        fun published() = cache.view.value.let { v -> mapOf(
            "members" to encoded(v.members), "items" to encoded(v.items), "occurrences" to encoded(v.occurrences), "board" to encoded(v.board), "agenda" to encoded(v.agenda),
            "reminders" to encoded(v.reminders), "notifications" to encoded(v.notifications), "prefs" to encoded(listOfNotNull(v.notificationPrefs)), "availability" to encoded(v.availability),
            "suggestions" to encoded(v.suggestions), "feeds" to encoded(v.sources), "lists" to encoded(v.lists), "comments" to encoded(v.comments), "attachments" to encoded(v.attachments),
            "settings" to encoded(listOfNotNull(v.settings)), "writes" to encoded(v.itemWrites.values.toList())) }
        fun close() { data.close(); keeper.close() }
        companion object { var nextDatabase = 0 }
    }
    private suspend fun TestScope.transition(seed: Int, member: Int, index: Int, fault: Int? = null, restart: Boolean = false, device: Device? = null, steps: JsonArray = recording(seed).getValue("steps").jsonArray) {
        val previous = steps[index - 1].jsonObject
        val current = steps[index].jsonObject
        val target = device ?: Device(this, previous, member)
        val full = Device(this, current, member)
        try {
            if (device == null) target.full()
            if (restart) target.restart()
            target.step = current
            target.faultStatus = fault
            target.delayed = fault != null
            val before = target.cache.sync.value?.cursor
            try { target.deliver(duplicate = true) }
            catch (e: Exception) {
                if (!target.faulted && e !is StaleVisibilityReply) throw e
                assertTrue((target.cache.sync.value?.cursor ?: 0) >= (before ?: 0))
                if (target.faulted) assertEquals(before, target.cache.sync.value?.cursor, "A failed first bundle must leave the acknowledgement prefix blocked")
                delay(2000)
                target.faultStatus = null
                target.deliver(duplicate = true)
            }
            if (fault != null) assertTrue(target.faulted, "Fault must reach a recorded bundle request")
            full.full()
            val actual = target.tables()
            for ((table, expected) in full.tables()) {
                val missing = expected - actual.getValue(table).toSet()
                val extra = actual.getValue(table) - expected.toSet()
                assertTrue(missing.isEmpty() && extra.isEmpty(), "seed=$seed member=$member step=$index op=${current["operation"]} table=$table missing=${missing.take(2)} extra=${extra.take(2)}")
            }
            assertEquals(full.published(), target.published(), "seed=$seed member=$member step=$index published cache")
            assertEquals(full.cache.view.value.agendaWindows, target.cache.view.value.agendaWindows)
            val visible = full.cache.view.value.items.mapTo(mutableSetOf()) { it.id }
            assertTrue(target.cache.view.value.items.all { it.id in visible })
            assertTrue(target.cache.view.value.comments.all { it.itemId in visible })
        } finally { if (device == null) target.close(); full.close() }
    }
    private fun campaign(seed: Int) = runTest(timeout = 180_000.milliseconds) {
        val failures = mutableListOf<String>()
        val steps = recording(seed).getValue("steps").jsonArray
        val devices = (0..1).map { Device(this, steps.first().jsonObject, it) }
        try {
            devices.forEach { it.full() }
            for (index in 1 until steps.size) for (member in 0..1) {
                try { transition(seed, member, index, restart = index % 9 == 0, device = devices[member], steps = steps) }
                catch (e: CancellationException) { throw e }
                catch (e: Throwable) { failures += "seed=$seed member=$member step=$index ${e.message}" }
            }
            failures.forEach(::println)
            assertTrue(failures.isEmpty(), "Recorded differential failures (${failures.size}):\n${failures.joinToString("\n")}")
        } finally { devices.forEach { it.close() } }
    }
    @Test fun realServerSeed1575937() = campaign(1575937)
    @Test fun realServerSeed1575938() = campaign(1575938)
    @Test fun seed1575937Member0Purge() = runTest { transition(1575937, 0, 45) }
    @Test fun seed1575938Member0Purge() = runTest { transition(1575938, 0, 45) }
    @Test fun recorded429And503RetryAfterDelayAndRestart() = runTest {
        for (status in listOf(429, 503)) transition(1575937, 0, 6, fault = status, restart = true)
    }
}
