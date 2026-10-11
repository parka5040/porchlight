package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.io.IOException
import java.nio.file.*
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.test.*

class AuditFinalClientCPreSendTest {
    @Test fun serverAlertRetriesAfterInFlightWriteFails() = storageFailure(false, false)
    @Test fun firedReminderRetriesAfterInFlightWriteFails() = storageFailure(true, false)
    @Test fun serverAlertUndoesPossiblyCommittedMarker() = storageFailure(false, true)
    @Test fun firedReminderUndoesPossiblyCommittedMarker() = storageFailure(true, true)
    private fun storageFailure(local: Boolean, committed: Boolean) = runBlocking {
        val bus = AuditFinalClientCNotifyBus()
        val damaged = AtomicBoolean(true)
        val failed = CompletableDeferred<Unit>()
        var beforeAttempt: DeliveryRecord? = null
        val writer: (Path, String) -> Unit = { file, text ->
            val records: List<DeliveryRecord> = WireJson.decodeFromJsonElement(
                WireJson.parseToJsonElement(text).jsonObject["records"] ?: JsonArray(emptyList()))
            if (damaged.get() && records.any { it.inFlight }) {
                if (committed) writeDesktopDelivery(file, text)
                failed.complete(Unit)
                throw IOException("In-flight inventory write unavailable")
            }
            if (!failed.isCompleted) beforeAttempt = records.singleOrNull()
            writeDesktopDelivery(file, text)
        }
        AuditFinalClientCNotifyRuntime(bus, writer).use { f ->
            f.start(); f.install(local)
            withTimeout(5_000) { failed.await() }
            assertEquals(0, bus.attempts.get(), "Failure must occur at persistence, before native submission")
            if (local) assertTrue(DesktopAlarmStore(f.dir).load().all().single().delivered, "Reminder must already be fired")
            if (committed) f.eventually("Unsent marker must be durably undone before retry") { f.records().singleOrNull() == beforeAttempt }
            damaged.set(false)
            f.cache.replaceNotifications(if (local) emptyList() else listOf(notificationFixture()))
            f.eventually("Repaired in-flight storage must retry the unsent delivery in the same runtime") { f.records().singleOrNull()?.posted == true }
            assertFalse(f.records().single().inFlight)
            assertEquals(1, bus.attempts.get())
            assertEquals(1, bus.posts)
        }
    }
}
