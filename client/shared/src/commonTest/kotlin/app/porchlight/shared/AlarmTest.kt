package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import app.porchlight.shared.api.models.ScheduledReminderRead
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.minutes

class AlarmTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun requestCodesNeverHashIdsAndSnoozeFencesStaleGenerations() {
        val ledger = AlarmLedger()
        val first = ledger.schedule("one", now, "item", "occ")
        val other = ledger.schedule("two", now, "item", "occ")
        assertEquals(1, first.code); assertEquals(2, other.code)
        val snooze = ledger.snooze("one", now + 15.minutes)
        val newer = ledger.snooze("one", now + 30.minutes)
        assertFalse(ledger.canFire(first.code, first.generation))
        assertFalse(ledger.canFire(snooze.code, snooze.generation))
        assertTrue(ledger.canFire(newer.code, newer.generation))
        ledger.fired(newer.code, newer.generation)
        assertFalse(ledger.canFire(newer.code, newer.generation))
        val restored = AlarmLedger.decode(ledger.encode())
        assertEquals(3, restored.schedule("three", now, "item", "occ").code)
    }
    @Test fun persistedSnoozeRetryIsIdempotentAndStaleGenerationIsRejected() {
        val ledger = AlarmLedger(); val original = ledger.schedule("r", now, "i", "o")
        ledger.fired(original.code, original.generation)
        val at = now + 15.minutes
        val first = ledger.snooze(original.code, original.generation, at)
        assertNotNull(first)
        val restored = AlarmLedger.decode(ledger.encode())
        assertEquals(first, restored.snooze(original.code, original.generation, at)) // same Worker replay
        val newer = restored.snooze(first.code, first.generation, now + 30.minutes)
        assertNotNull(newer)
        assertNull(restored.snooze(original.code, original.generation, at)) // an older Worker cannot undo it
        restored.cancel("r")
        assertNull(restored.snooze(newer.code, newer.generation, now + 45.minutes))
    }
    @Test fun sevenDayPlanIsPrivateAndRetainsPendingAlarmsUntilDerivation() = runTest {
        val cache = memoryCache { now }
        try {
            val r = ScheduledReminderRead("r", "h", "self", "o", "one", "offset", now + 1.days, now)
            cache.install(emptySnapshot(1).copy(items=listOf(itemRead("one")), occurrences=listOf(occurrenceRead("o","one")),
                scheduledReminders=listOf(r, r.copy(id="far",fireAt=now+8.days), r.copy(id="other",memberId="other"))))
            val ledger = AlarmLedger(); reconcileAlarms(ledger, cache.view.value, now)
            assertEquals(listOf("r"), ledger.active().map { it.key })
            cache.install(emptySnapshot(2).copy(items=listOf(itemRead("one", true)), occurrences=listOf(occurrenceRead("o","one"))))
            reconcileAlarms(ledger, cache.view.value, now)
            assertEquals(listOf("r"), ledger.active().map { it.key })
            cache.install(emptySnapshot(3).copy(items=listOf(itemRead("one")), occurrences=listOf(occurrenceRead("o","one"))))
            reconcileAlarms(ledger, cache.view.value, now); assertTrue(ledger.active().isEmpty())
        } finally { cache.close() }
    }
}
