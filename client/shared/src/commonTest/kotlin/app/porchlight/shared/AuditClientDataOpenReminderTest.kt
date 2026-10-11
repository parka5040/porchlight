package app.porchlight.shared

import app.porchlight.shared.api.models.ScheduledReminderRead
import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

class AuditClientDataOpenReminderTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun backlogArmsRetainsAndDeliversButClosedStatusesCancel() = runTest {
        val cache = memoryCache { now }
        try {
            val ledger = AlarmLedger()
            val deliveries = DeliveryInventory()
            for (status in listOf("todo", "backlog", "doing", "done", "skipped", "backlog")) {
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")),
                    occurrences = listOf(occurrenceRead("o", "one").copy(status = status)),
                    scheduledReminders = listOf(ScheduledReminderRead("r", "h", "self", "o", "one", "offset", now + 1.days, now))))
                reconcileAlarms(ledger, cache.view.value, now)
                val open = status != "done" && status != "skipped"
                assertEquals(open, ledger.active().isNotEmpty(), "alarm status=$status")
                deliveries.put(DeliveryRecord("r", 1, localAlarm = true, itemId = "one", occurrenceId = "o"))
                assertEquals(open, deliveries.localToPost(cache.view.value).isNotEmpty(), "delivery status=$status")
                assertEquals(!open, deliveries.toCancel(cache.view.value).isNotEmpty(), "cancel status=$status")
            }
        } finally { cache.close() }
    }
}
