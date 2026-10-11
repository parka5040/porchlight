package app.porchlight.shared.data

import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataFix1FenceChurnTest {
    @Test fun historyIsBoundedDuringChurnAndRestartWithoutReleasingOldReaders() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-churn-").toFile()
        val file = root.resolve("cache.db")
        val driver = openDesktopCache(file)
        val cache = ReadCache(driver) { now }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        try {
            cache.initialize()
            val old = async {
                assertFailsWith<StaleVisibilityReply> { cache.inSession {
                    started.complete(Unit); release.await(); cache.putItem(itemResponse("old"))
                } }
            }
            started.await(); cache.evictItem("old")
            for (i in 1..600) { cache.evictItem("gone-$i"); cache.inSession { cache.putItem(itemResponse("gone-$i")) } }
            val q = PorchlightDb(driver).cacheQueries
            assertTrue(q.allVisibility().executeAsList().size <= 256, "visibility history must have a bounded lifecycle")
            assertTrue(cache.view.value.evictedItemIds.size <= 256, "published tombstones must also be bounded")
            cache.inSession { cache.putItem(itemResponse("old").copy(version = 2)) }
            release.complete(Unit); old.await()
            cache.close()
            val reopenedDriver = openDesktopCache(file)
            val reopened = ReadCache(reopenedDriver) { now }
            try {
                reopened.initialize()
                assertTrue(PorchlightDb(reopenedDriver).cacheQueries.allVisibility().executeAsList().size <= 256)
                reopened.inSession { reopened.putItem(itemResponse("new")) }
                assertTrue(reopened.view.value.items.any { it.id == "new" })
            } finally { reopened.close() }
        } finally { release.complete(Unit); cache.close(); root.deleteRecursively() }
    }
}
