package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.hours

class PrerolloutClientSyncFixACleanupTest {
    private val f = SyncBundleFixtures
    private class Store(private val failureKind: String = "both") : AttachmentStore {
        val files = mutableSetOf<String>()
        val views = mutableSetOf<String>()
        var failing = true
        override suspend fun read(key: String): ByteArray? = if (key in files) byteArrayOf(1) else null
        override suspend fun write(key: String, bytes: ByteArray) { files += key }
        override suspend fun clear() { files.clear(); views.clear() }
        override suspend fun remove(key: String) { if (failing && failureKind != "view") error("purge unavailable"); files -= key }
        override suspend fun removeView(id: String) { if (failing && failureKind != "file") error("viewer purge unavailable"); views -= id }
        override suspend fun cleanup(before: Instant) = Unit
    }

    @Test fun snapshotWatermarkWaitsForCleanupAndAnotherOwnerClearsOldObligations() = runTest {
        val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(driver)
        val sql = PorchlightDb(driver).cacheQueries
        val store = Store()
        var now = f.now
        val cache = ReadCache({ driver }, { now }, null, store)
        try {
            cache.initialize(CacheOwner("fixture-ca", "member-a", "device-a"))
            cache.install(f.snapshot)
            val row = f.snapshot.attachments.single()
            store.files += attachmentKey(cache.attachmentNamespace, row, false); store.views += row.id
            assertFailsWith<IllegalStateException> { cache.install(f.snapshot.copy(cursor = 900, attachments = emptyList())) }
            assertEquals(f.snapshot.cursor, sql.getSync().executeAsOne().cursor)
            assertEquals(f.snapshot.cursor, cache.sync.value?.cursor)
            assertEquals(900, sql.getPendingSync().executeAsOne().cursor)
            assertTrue(sql.allAttachmentCleanup().executeAsList().isNotEmpty())
            store.failing = false; now += 1.hours
            cache.initialize(CacheOwner("fixture-ca", "member-a", "device-a"))
            assertEquals(900, cache.sync.value?.cursor)
            assertTrue(sql.allAttachmentCleanup().executeAsList().isEmpty())
            assertTrue(store.files.isEmpty()); assertTrue(store.views.isEmpty())

            cache.install(f.snapshot)
            store.files += attachmentKey(cache.attachmentNamespace, row, false); store.views += row.id; store.failing = true
            assertFailsWith<IllegalStateException> { cache.evictAttachment(row.id) }
            now += 1.hours
            cache.initialize(CacheOwner("fixture-ca", "member-b", "device-b"))
            assertTrue(sql.allAttachmentCleanup().executeAsList().isEmpty())
            assertNull(sql.getPendingSync().executeAsOneOrNull())
            assertTrue(cache.view.value.attachments.isEmpty())
            assertTrue(store.files.isEmpty()); assertTrue(store.views.isEmpty())
        } finally { cache.close() }
    }

    @Test fun replayCannotAcknowledgeFailedPurgesAndRestartRetriesDurableObligations() = runTest {
        for (restart in listOf(false, true)) for (failureKind in listOf("file", "view")) {
            val root = File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }
            val dir = Files.createTempDirectory(root.toPath(), "sync-cleanup-").toFile()
            val file = File(dir, "read-cache.db")
            val store = Store(failureKind)
            var now = f.now
            fun open(): ReadCache = ReadCache({ JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath) }, { now }, null, store)
            JdbcSqliteDriver("jdbc:sqlite:" + file.absolutePath).let { PorchlightDb.Schema.create(it); it.close() }
            var cache = open()
            try {
                cache.install(f.snapshot)
                val row = f.snapshot.attachments.single()
                store.files += attachmentKey(cache.attachmentNamespace, row, false); store.files += attachmentKey(cache.attachmentNamespace, row, true); store.views += row.id
                val page = f.bundle().copy(attachments = emptyList())
                assertFailsWith<IllegalStateException> { cache.installBundle(page) }
                assertTrue(cache.view.value.attachments.isEmpty())
                if (restart) { cache.close(); cache = open(); assertFailsWith<IllegalStateException> { cache.initialize() } }
                assertFailsWith<IllegalStateException> { cache.installBundle(page); cache.acknowledgeCursor(999) }
                assertFailsWith<IllegalStateException> { cache.acknowledgeCursor(999) }
                assertNotEquals(999, cache.sync.value?.cursor)
                assertTrue(if (failureKind == "file") store.files.isNotEmpty() else store.views.isNotEmpty())
                store.failing = false; now += 1.hours
                if (restart) { cache.close(); cache = open(); cache.initialize() }
                cache.installBundle(page); cache.acknowledgeCursor(999)
                assertEquals(999, cache.sync.value?.cursor)
                assertTrue(store.files.isEmpty()); assertTrue(store.views.isEmpty())
            } finally { cache.close(); dir.deleteRecursively() }
        }
    }
}
