package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.api.models.ConflictRead
import app.porchlight.shared.screens.calendar.calendarConflictLabel
import app.porchlight.shared.transport.*
import app.porchlight.shared.live.*
import app.porchlight.shared.api.models.SnapshotRead
import kotlinx.serialization.json.*
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.*
import kotlinx.coroutines.test.*
import kotlinx.datetime.TimeZone
import kotlinx.serialization.json.JsonObject
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncFixERecoveryTest {
    private val f = SyncBundleFixtures
    private class Store : AttachmentStore {
        val files = mutableSetOf<String>()
        val views = mutableSetOf<String>()
        var fail = false
        var clearFail = false
        var attempts = 0
        override suspend fun read(key: String) = if (key in files) byteArrayOf(1) else null
        override suspend fun write(key: String, bytes: ByteArray) { files += key }
        override suspend fun clear() { if (clearFail) error("Storage unavailable"); files.clear(); views.clear() }
        override suspend fun remove(key: String) { attempts++; if (fail) error("Storage unavailable"); files -= key }
        override suspend fun removeView(id: String) { views -= id }
        override suspend fun cleanup(before: Instant) = Unit
    }
    private fun directory() = Files.createTempDirectory(File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }.toPath(), "sync-recovery-").toFile()
    private fun transport() = object : ApiTransport {
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = error("Unexpected request")
        override suspend fun download(path: String, session: String?) = byteArrayOf(1)
    }

    @Test fun sameOwnerRestartPurgesOriginalAndThumbnailFromPriorProcessEpoch() = runTest {
        val dir = directory(); val file = File(dir, "cache.db"); val store = Store()
        JdbcSqliteDriver("jdbc:sqlite:$file").let { PorchlightDb.Schema.create(it); it.close() }
        fun open() = ReadCache({ JdbcSqliteDriver("jdbc:sqlite:$file") }, { f.now }, null, store)
        var cache = open()
        val owner = CacheOwner("fixture-ca", "member-a", "device-a")
        val row = f.snapshot.attachments.single().copy(hasThumbnail = true)
        try {
            cache.sessionEpoch = { 1 }; cache.initialize(owner); cache.install(f.snapshot.copy(attachments = listOf(row)))
            val repository = AttachmentRepository(GeneratedCall({ transport() }, { "fixture-session" }), cache, WriteCoordinator(), { f.now })
            repository.download(row); repository.download(row, true); store.views += row.id
            assertEquals(2, store.files.size)
            val namespace = cache.attachmentNamespace
            cache.close(); cache = open(); cache.sessionEpoch = { 0 }; cache.initialize(owner)
            assertEquals(namespace, cache.attachmentNamespace)
            cache.evictAttachment(row.id)
            assertTrue(store.files.isEmpty(), "Prior-process originals and thumbnails must disappear before acknowledgement")
            assertTrue(store.views.isEmpty(), "Viewer aliases must disappear before acknowledgement")
            cache.acknowledgeCursor(999)
            cache.initialize(owner.copy(memberId = "member-b"))
            assertNotEquals(namespace, cache.attachmentNamespace, "Replacement identity gets a fresh persisted namespace")
        } finally { cache.close(); dir.deleteRecursively() }
    }

    @Test fun permanentCleanupFailureIsVisibleAndRetryWorkIsBounded() = runTest {
        val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY); PorchlightDb.Schema.create(driver)
        val store = Store(); val cache = ReadCache({ driver }, { f.now }, null, store)
        try {
            cache.install(f.snapshot)
            store.fail = true
            assertFailsWith<IllegalStateException> { cache.evictAttachment(f.snapshot.attachments.single().id) }
            assertNotNull(cache.storageNotice.value, "Admitted owners need an actionable storage notice")
            repeat(20) { assertFailsWith<IllegalStateException> { cache.acknowledgeCursor(999) } }
            assertTrue(store.attempts <= 2, "Cleanup retries must back off, not repeat every stream event")
            assertNotEquals(999, cache.sync.value?.cursor)
        } finally { cache.close() }
    }

    @Test fun replacementIdentityDoesNotWaitForOldCooldownOrHoldSnapshotPermits() = runTest { replacementSession(true) }
    @Test fun replacementTokenDoesNotWaitForOldCooldownOrHoldSnapshotPermits() = runTest { replacementSession(false) }
    @Test fun snapshotOnlyIdentityReplacementWakesOldCooldown() = runTest { replacementSession(true, snapshotOnly = true) }
    @Test fun snapshotOnlyTokenReplacementWakesOldCooldown() = runTest { replacementSession(false, snapshotOnly = true) }

    private suspend fun TestScope.replacementSession(replaceIdentity: Boolean, snapshotOnly: Boolean = false) {
        val cache = memoryCache { f.now }; var epoch = 0L; var revision = 0L
        cache.sessionEpoch = { epoch }; cache.initialize()
        val call = GeneratedCall({ error("No HTTP needed") }, { "fixture-session" }, sessionRevision = { revision })
        val slots = Semaphore(2); val lane = Mutex(); var reads = 0
        val workers = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val scheduler = TargetedSyncScheduler(cache, call, workers, slots, { _, _ ->
            if (++reads == 1) throw ApiProblem("retry", 503, "Retry", retryAfterMillis = 300_000)
            false
        }, {}, { f.now })
        scheduler.batchMillis = 0
        val sleeping = CompletableDeferred<Unit>()
        scheduler.waitingForCooldown = { sleeping.complete(Unit) }
        scheduler.orderRead = { _, operation -> lane.withLock { operation() } }
        try {
            assertFailsWith<ApiProblem> { scheduler.request(listOf(SyncDemand(SyncResource.Item("first")))) }
            val old = async { runCatching { scheduler.request(listOf(SyncDemand(SyncResource.Item("sleeping")))) } }
            sleeping.await()
            assertEquals(2, slots.availablePermits, "Cooldown waits must be outside shared read permits")
            assertTrue(lane.tryLock(), "Cooldown waits must be outside content lanes"); lane.unlock()
            if (replaceIdentity) epoch++ else revision++
            // The trust binding notifies the scheduler even if the replacement only starts a snapshot.
            if (snapshotOnly) scheduler.sessionChanged()
            val replacement = async { if (!snapshotOnly) scheduler.request(listOf(SyncDemand(SyncResource.Item("replacement")))) }
            val snapshot = async { slots.withPermit { slots.withPermit { } } }
            runCurrent()
            // SQLite runs on a real IO dispatcher; bound real time without advancing the five-minute virtual sleep.
            withContext(Dispatchers.Default) { withTimeout(2_000) {
                replacement.await(); snapshot.await(); old.await()
            } }
            assertTrue(replacement.isCompleted, "Replacement content must not wait five minutes for the old lane")
            assertTrue(snapshot.isCompleted, "Old cooldown must release both snapshot permits")
            assertTrue(old.isCompleted, "Identity replacement must wake stale cooldown sleepers")
            assertIs<CancellationException>(old.await().exceptionOrNull(), "Old-session work must be cancelled, not read under the replacement")
            assertEquals(if (snapshotOnly) 1 else 2, reads)
            assertTrue(testScheduler.currentTime < 300_000, "Replacement must not advance through the old cooldown")
        } finally { workers.cancel(); cache.close() }
    }

    @Test fun durableBackoffAndExplicitRecoveryWipeRevokedArtifactsBeforeServerRefresh() = runTest {
        val dir = directory(); val database = File(dir, "cache.db"); val files = File(dir, "attachments")
        JdbcSqliteDriver("jdbc:sqlite:$database").let { PorchlightDb.Schema.create(it); it.close() }
        val disk = FileAttachmentStore(files)
        var removals = 0; var failClear = false; var at = f.now
        val store = object : AttachmentStore by disk {
            override suspend fun remove(key: String) { removals++; error("Permanent removal failure") }
            override suspend fun clear() { if (failClear) error("Directory unavailable"); disk.clear() }
        }
        fun open() = ReadCache({ JdbcSqliteDriver("jdbc:sqlite:$database") }, { at }, null, store)
        var cache = open()
        val row = f.snapshot.attachments.single().copy(hasThumbnail = true)
        val wire = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when {
                path.startsWith("/v1/snapshot") -> WireJson.encodeToJsonElement(f.snapshot.copy(cursor = 999, attachments = emptyList(), lists = emptyList(), comments = emptyList())).jsonObject
                path.startsWith("/v1/board") || path.startsWith("/v1/agenda") -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> error("Unexpected request")
            }
            override suspend fun download(path: String, session: String?) = byteArrayOf(1)
        }
        try {
            cache.install(f.snapshot.copy(attachments = listOf(row)))
            val repository = AttachmentRepository(GeneratedCall({ wire }, { "fixture-session" }), cache, WriteCoordinator(), { at })
            repository.download(row); repository.download(row, true)
            File(files, "${row.id}-view.pdf").writeBytes(byteArrayOf(1))
            assertFailsWith<IllegalStateException> { cache.evictAttachment(row.id) }
            assertTrue(files.listFiles()!!.isNotEmpty())
            val live = LiveSync(StreamSource { _, receive ->
                receive(StreamFrame.Heartbeat)
                receive(StreamFrame.Tickle(f.snapshot.cursor + 1, "notification.updated", "notification", "unrelated", false))
            }, { cache.sync.value?.cursor }, {
                cache.inSession { fence -> cache.replaceNotificationIds(fence, setOf("unrelated"), emptyList()) }
            }, { cache.acknowledgeCursor(it) }, {}, { at })
            live.progressBlocked = { cache.cleanupRetryAt.value != null }
            assertFailsWith<IllegalStateException> { live.connectOnce() }
            assertNotNull(live.unreachableSince.value); assertFalse(live.hasRecentStreamFrame())
            assertNotEquals(ConnectionState.Connected, live.state.value)
            assertEquals(ATTACHMENT_CLEANUP_NOTICE, cache.storageNotice.value, "Unrelated commits must not clear the storage notice")
            var retryAt = checkNotNull(cache.cleanupRetryAt.value)
            assertEquals(60_000, (retryAt - at).inWholeMilliseconds)
            repeat(7) { attempt ->
                val count = removals
                repeat(10) { assertFailsWith<IllegalStateException> { cache.acknowledgeCursor(999) } }
                assertEquals(count, removals, "Backoff must bound retry work across unrelated deliveries")
                cache.close(); cache = open()
                assertFailsWith<IllegalStateException> { cache.initialize() }
                assertEquals(count, removals, "Restart must preserve the retry deadline")
                assertEquals(retryAt, cache.cleanupRetryAt.value)
                at = retryAt
                assertFailsWith<IllegalStateException> { cache.retryAttachmentCleanup() }
                assertEquals(count + 2, removals)
                retryAt = checkNotNull(cache.cleanupRetryAt.value)
                assertEquals((120_000L shl attempt).coerceAtMost(3_600_000), (retryAt - at).inWholeMilliseconds)
            }
            val data = HouseholdData(cache, GeneratedCall({ wire }, { "fixture-session" }), { at })
            failClear = true
            assertFailsWith<IllegalStateException> { data.clearLocalCache() }
            assertFailsWith<IllegalStateException> { cache.acknowledgeCursor(999) }
            assertTrue(files.listFiles()!!.isNotEmpty(), "A failed wipe must retain its obligation")
            failClear = false
            data.clearLocalCache()
            assertTrue(files.listFiles()!!.isEmpty(), "No revoked original, thumbnail or viewer may survive recovery")
            assertTrue(cache.view.value.attachments.isEmpty())
            assertEquals(999, cache.sync.value?.cursor, "Recovery must refresh from the server")
            assertNull(cache.storageNotice.value); assertNull(cache.cleanupRetryAt.value)
            data.close()
        } finally { cache.close(); dir.deleteRecursively() }
    }

    @Test fun firstOpenAfterUpgradePurgesLegacyEpochFilesBeforeAdmittingOwner() = runTest {
        val dir = directory(); val files = File(dir, "attachments").apply { mkdirs() }
        val database = File(dir, "cache.db")
        val row = f.snapshot.attachments.single()
        listOf(attachmentKey(0, row, false), attachmentKey(1, row, true), "${row.id}-view.pdf").forEach { File(files, it).writeBytes(byteArrayOf(1)) }
        // Reproduce the platform's schema-13 mismatch path, not merely an empty current-schema database.
        JdbcSqliteDriver("jdbc:sqlite:$database").let { driver ->
            PorchlightDb.Schema.create(driver)
            PorchlightDb(driver).cacheQueries.putSync(800, f.now.toEpochMilliseconds())
            driver.execute(null, "DROP TABLE CacheFileNamespace", 0)
            driver.execute(null, "DROP TABLE CacheCleanupFailure", 0)
            driver.execute(null, "PRAGMA user_version = 13", 0)
            driver.close()
        }
        assertEquals(14L, PorchlightDb.Schema.version)
        val disk = FileAttachmentStore(files)
        var failClear = true; var at = f.now
        val store = object : AttachmentStore by disk {
            override suspend fun clear() { if (failClear) error("Directory unavailable"); disk.clear() }
        }
        fun open() = desktopReadCache(database, store) { at }
        val owner = CacheOwner("fixture-ca", "member-a", "device-a")
        var cache = open()
        try {
            assertFailsWith<IllegalStateException> { cache.initialize(owner) }
            assertTrue(files.listFiles()!!.isNotEmpty(), "A failed upgrade purge must not admit legacy files")
            assertEquals(0L, cache.attachmentNamespace)
            assertEquals(ATTACHMENT_CLEANUP_NOTICE, cache.storageNotice.value)
            cache.close(); cache = open()
            assertFailsWith<IllegalStateException> { cache.initialize(owner) }
            failClear = false; at += 1.days
            cache.initialize(owner)
            assertTrue(files.listFiles()!!.isEmpty(), "Upgrade recovery must remove every legacy original, thumbnail and viewer")
            assertNull(cache.sync.value, "Schema-13 household state must be rebuilt")
            assertTrue(cache.attachmentNamespace > 0)
            assertNull(cache.storageNotice.value); assertNull(cache.cleanupRetryAt.value)
            val namespace = cache.attachmentNamespace
            cache.close(); cache = open(); cache.initialize(owner)
            assertEquals(namespace, cache.attachmentNamespace)
        } finally { cache.close(); dir.deleteRecursively() }
    }

    @Test fun visibilityLossRetainsBusyWarningInSqlAndCalendar() = runTest {
        val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY); PorchlightDb.Schema.create(driver)
        val cache = ReadCache(driver) { f.now }
        val peer = f.snapshot.items.single().copy(id = "private-peer", title = "Private appointment")
        val conflict = ConflictRead(f.snapshot.memberSettings.memberId, f.agenda.agenda!!.occurrenceId,
            peer.title, f.now, f.now + 1.days, "hard", otherItemId = peer.id)
        try {
            cache.install(f.snapshot.copy(items = f.snapshot.items + peer), projections = Projections(emptyList(), AgendaWindow(f.from, f.to, null, listOf(f.agenda.copy(conflicts = listOf(conflict))))))
            cache.evictItem(peer.id)
            val busy = cache.view.value.agenda.single().conflicts.orEmpty().single()
            assertNull(busy.otherItemId); assertNull(busy.otherOccurrenceId); assertEquals("Busy", busy.otherTitle)
            assertEquals(conflict.startsAt, busy.startsAt); assertEquals(conflict.kind, busy.kind)
            assertTrue("Busy" in calendarConflictLabel(busy, TimeZone.UTC))
            val sql = PorchlightDb(driver).cacheQueries.allAgendas().executeAsOne().payload
            assertTrue("Busy" in sql); assertFalse(peer.title in sql); assertFalse(peer.id in sql)
        } finally { cache.close() }
    }
}
