package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncFixDVisibilityTest {
    private val f = SyncBundleFixtures

    @Test fun notificationRepairCrossingEvictionsRereadsWithoutPublishingOldReply() = runTest {
        val cache = memoryCache { f.now }; cache.install(f.snapshot)
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val item = f.snapshot.items.single().id
        val notice = Notification("notice", "activity", "activity", "item", item, 1,
            NotificationSummary("Private title", "Private title", "Private title", "activity", false), f.now, f.now, 1, itemId = item, releasedAt = f.now)
        var reads = 0
        val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { fence, _ ->
            reads++
            if (reads == 1) { started.complete(Unit); release.await() }
            assertTrue(cache.view.value.notifications.isEmpty())
            cache.replaceNotificationIds(fence, setOf(notice.id), if (reads == 1) listOf(notice) else emptyList())
            false
        }, { error("Narrow repair must not need a snapshot") })
        try {
            val ordinary = async { cache.inSession { fence ->
                started.await(); release.await()
                assertFailsWith<StaleVisibilityReply> { cache.replaceNotificationIds(fence, setOf(notice.id), listOf(notice)) }
            } }
            val repair = async { scheduler.request(listOf(SyncDemand(SyncResource.Notification(notice.id)))) }
            started.await()
            cache.evictItem(item)
            cache.evictItem(item)
            release.complete(Unit)
            repair.await(); ordinary.await()
            assertEquals(2, reads)
            assertTrue(cache.view.value.notifications.isEmpty())
            assertTrue(item in cache.view.value.evictedItemIds)
        } finally { release.complete(Unit); cache.close() }
    }

    @Test fun retriesAreBoundedAndRequireANewerVisibilityFence() = runTest {
        for (advanceFence in listOf(false, true)) {
            val cache = memoryCache { f.now }; cache.initialize()
            var reads = 0
            val scheduler = TargetedSyncScheduler(cache, null, backgroundScope, Semaphore(2), { fence, _ ->
                reads++
                if (advanceFence) {
                    cache.evictItem("hidden-$reads")
                    cache.replaceNotificationIds(fence, setOf("notice"), emptyList())
                }
                throw StaleVisibilityReply()
            }, {})
            try {
                assertFailsWith<StaleVisibilityReply> { scheduler.request(listOf(SyncDemand(SyncResource.Notification("notice")))) }
                assertEquals(if (advanceFence) 4 else 1, reads, "Only newer fences permit the three bounded re-reads")
            } finally { cache.close() }
        }
    }

    @Test fun targetedEvictionDoesNotInvalidateInboxTwiceForOneItemFence() = runTest {
        for (stream in listOf(false, true)) {
            val expected = memoryCache { f.now }; expected.install(f.snapshot)
            val cache = memoryCache { f.now }; cache.install(f.snapshot)
            val item = f.snapshot.items.single().id
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    if (path.startsWith("/v1/items/")) throw ApiProblem("not-found", 404, "Hidden item")
                    assertTrue(path.startsWith("/v1/notifications"))
                    return buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
                }
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }, jsonDispatcher = StandardTestDispatcher(testScheduler)),
                { f.now }, syncDispatcher = StandardTestDispatcher(testScheduler))
            data.itemSyncCapability = { true }; data.targetedSyncVerified = true
            try {
                expected.evictItem(item)
                if (stream) data.applyStreamFrame(StreamFrame.Tickle(100, "item.access_changed", "item", item, true))
                else cache.inSession { data.readTargeted(it, SyncDemand(SyncResource.Item(item))) }
                assertEquals(expected.view.value.visibilityRevision, cache.view.value.visibilityRevision,
                    "Item eviction already hides the inbox atomically; another hide invalidates repairs unnecessarily")
            } finally { data.close(); expected.close() }
        }
    }
}
