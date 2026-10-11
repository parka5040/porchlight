package app.porchlight.shared

import app.porchlight.shared.api.models.ScheduledReminderRead
import app.porchlight.shared.data.*
import app.porchlight.shared.live.AlarmLedger
import app.porchlight.shared.live.reconcileAlarms
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

class AuditFinalReminderIdentityTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun unchangedRowIdPreservesDueUndeliveredTicket() = runTest {
        checkReplacement(sameId = true)
    }

    /** Documents the old server failure: a replacement ID cannot admit a new overdue alarm. */
    @Test fun changedRowIdCancelsDueTicketWithoutRearming() = runTest {
        checkReplacement(sameId = false)
    }

    @Test fun splitReparentsFutureTicketWithoutReplacingNativeIdentity() = runTest {
        checkSplit(overdue = false)
    }

    @Test fun splitReparentsDueTicketEvenWhenHeadLeavesSnapshot() = runTest {
        checkSplit(overdue = true)
    }

    private suspend fun checkSplit(overdue: Boolean) {
        val cache = memoryCache { now }
        try {
            val reminder = ScheduledReminderRead("r", "h", "self", "o", "head", "offset", now + 1.minutes, now)
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("head")),
                occurrences = listOf(occurrenceRead("o", "head")), scheduledReminders = listOf(reminder)))
            val ledger = AlarmLedger()
            reconcileAlarms(ledger, cache.view.value, now, preserveOverdue = true)
            val original = ledger.active().single()
            val later = if (overdue) now + 2.minutes else now
            // A split moves the occurrence and stable reminder ID to the tail. The head can disappear independently.
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead("tail")),
                occurrences = listOf(occurrenceRead("o", "tail")),
                scheduledReminders = listOf(reminder.copy(itemId = "tail", derivedAt = later))))
            reconcileAlarms(ledger, cache.view.value, later, preserveOverdue = true)
            assertEquals(listOf(original.copy(itemId = "tail")), ledger.active(),
                "split must refresh the parent without cancelling or replacing the ticket")
            assertTrue(ledger.canFire(original.code, original.generation))
            val restored = AlarmLedger.decode(ledger.encode())
            assertEquals(ledger.active(), restored.active(), "reparenting survives process restart without migration")
        } finally { cache.close() }
    }

    private suspend fun checkReplacement(sameId: Boolean) {
        val cache = memoryCache { now }
        try {
            val reminder = ScheduledReminderRead("r", "h", "self", "o", "one", "offset", now + 1.minutes, now)
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")),
                occurrences = listOf(occurrenceRead("o", "one")), scheduledReminders = listOf(reminder))
            cache.install(snapshot)
            val ledger = AlarmLedger()
            reconcileAlarms(ledger, cache.view.value, now, preserveOverdue = true)
            val original = ledger.active().single()
            ledger.checkpoint()

            val later = now + 2.minutes
            cache.install(snapshot.copy(cursor = 2, scheduledReminders = listOf(
                reminder.copy(id = if (sameId) reminder.id else "replacement", derivedAt = later))))
            reconcileAlarms(ledger, cache.view.value, later, preserveOverdue = true)
            if (sameId) {
                assertEquals(listOf(original), ledger.active(), "same code, generation and undelivered obligation")
                assertTrue(ledger.changes().isEmpty(), "unchanged row must not cancel or rearm the native alarm")
                assertTrue(ledger.canFire(original.code, original.generation))
                ledger.fired(original.code, original.generation)
                assertFalse(ledger.canFire(original.code, original.generation), "retained ticket only fires once")
            } else {
                assertTrue(ledger.active().isEmpty(), "replacement already in the past cannot be armed")
                assertFalse(ledger.canFire(original.code, original.generation))
                assertFalse(ledger.ticket(original.code)!!.active)
                assertEquals(listOf(original.key), ledger.all().map { it.key })
            }
        } finally { cache.close() }
    }
}
