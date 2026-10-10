package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AlarmReceiverTest {
    @Test fun coldReceiverCacheCollectorRunsBeforeFireAndSnoozeStillWorks() = runTest {
        val due = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { due - 1.minutes }
        try {
            val reminder = app.porchlight.shared.api.models.ScheduledReminderRead("r", "h", "self", "o", "i", "offset", due, due - 1.minutes)
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("i")),
                occurrences = listOf(occurrenceRead("o", "i")), scheduledReminders = listOf(reminder)))
            val ledger = AlarmLedger()
            val ticket = ledger.schedule("r", due, "i", "o")
            val restored = AlarmLedger.decode(ledger.encode())
            val now = due + 1.minutes
            // get(AppServices) initializes the cache and wakes bindAndroidLive's collector first.
            reconcileAndroidAlarms(restored, cache.view.value, now)
            assertTrue(restored.canFire(ticket.code, ticket.generation), "startup reconcile must preserve the due ticket")
            restored.fired(ticket.code, ticket.generation)
            assertEquals(setOf("o"), firedOccurrences(restored.all()))
            // The Worker reconciles again before applying the notification's local Snooze.
            reconcileAndroidAlarms(restored, cache.view.value, now)
            val snoozed = assertNotNull(restored.snooze(ticket.code, ticket.generation, now + 15.minutes))
            assertEquals(now + 15.minutes, snoozed.fireAt)
            assertTrue(restored.canFire(snoozed.code, snoozed.generation))
            reconcileAndroidAlarms(restored, cache.view.value.copy(occurrences = emptyList()), now)
            assertTrue(restored.active().isEmpty())
        } finally { cache.close() }
    }
    @Test fun directReceiverDispatchToleratesNullPendingResultAndAwaitsWork() = runTest {
        var completed = false
        runAlarmReschedule(null) { completed = true }
        assertTrue(completed)
    }
}
