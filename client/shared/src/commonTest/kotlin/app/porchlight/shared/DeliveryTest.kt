package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class DeliveryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun restartDeliversUnreadCancelsCrossDeviceReadAndRetriesDeniedDelivery() = runTest {
        val cache = memoryCache { now }
        try {
            val n = notificationFixture()
            cache.install(emptySnapshot(1).copy(notifications = listOf(n)))
            val ledger = DeliveryInventory()
            assertEquals(listOf(n.id), ledger.serverToPost(cache.view.value).map { it.id }) // cold start unread
            ledger.put(DeliveryRecord(n.id, n.version.toLong())) // committed before OS notify, then permission denial
            val restarted = DeliveryInventory.decode(ledger.encode())
            assertEquals(1, restarted.serverToPost(cache.view.value).size)
            restarted.posted(n.id)
            val delivered = DeliveryInventory.decode(restarted.encode())
            assertTrue(delivered.serverToPost(cache.view.value).isEmpty())
            cache.install(emptySnapshot(2).copy(notifications = listOf(n.copy(readAt = now))))
            assertEquals(listOf(n.id), delivered.toCancel(cache.view.value).map { it.key })
            delivered.remove(n.id)
            assertTrue(DeliveryInventory.decode(delivered.encode()).records().isEmpty())
        } finally { cache.close() }
    }
    @Test fun remoteDismissCancelsPersistedLocalAlarmAndDesktopHandleSurvivesRestart() = runTest {
        val cache = memoryCache { now }
        try {
            val remote = notificationFixture().copy(occurrenceId = "o", summary = notificationFixture().summary.copy(localAlarm = true), readAt = now)
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("i")), occurrences = listOf(occurrenceRead("o", "i")), notifications = listOf(remote)))
            val ledger = DeliveryInventory()
            ledger.put(DeliveryRecord("alarm:3", 2, localAlarm = true, itemId = "i", occurrenceId = "o"))
            ledger.posted("alarm:3", 123)
            val restored = DeliveryInventory.decode(ledger.encode())
            assertEquals(123, restored.get("alarm:3")?.handle)
            assertEquals(listOf("alarm:3"), restored.toCancel(cache.view.value).map { it.key })
        } finally { cache.close() }
    }
    @Test fun deliveredLocalAlarmIsCancelledOnRemoteCompleteAndLocalDismissIsNotReposted() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("i")), occurrences = listOf(occurrenceRead("o", "i"))))
            val ledger = DeliveryInventory()
            ledger.put(DeliveryRecord("alarm:3", 2, localAlarm = true, itemId = "i", occurrenceId = "o"))
            ledger.posted("alarm:3"); ledger.dismissed("alarm:3")
            val restarted = DeliveryInventory.decode(ledger.encode())
            assertTrue(restarted.localToPost(cache.view.value).isEmpty())
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead("i")), occurrences = listOf(occurrenceRead("o", "i").copy(status = "done"))))
            assertEquals(listOf("alarm:3"), restarted.toCancel(cache.view.value).map { it.key })
        } finally { cache.close() }
    }
}
