package app.porchlight.android

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.porchlight.shared.settings.PrivateMetadataStore
import android.os.Handler
import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.android.trust.ControllerOnly
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheOwner
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.data.SyncState
import app.porchlight.shared.live.*
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.settings.privateMetadata
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.FileInputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

/** Disposable, unenrolled API35/36 controller AVD only. Real manifest receiver dispatch (including goAsync),
 * AlarmManager calls and preference commits; no helper-only substitute and no enrollment/network needed. */
@ControllerOnly
@RunWith(AndroidJUnit4::class)
class AuditBlClient3bAlarmInstrumentedTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val at = Instant.parse("2035-01-01T00:00:00Z")
    private fun shell(command: String) = instrumentation.uiAutomation.executeShellCommand(command).use {
        FileInputStream(it.fileDescriptor).bufferedReader().use { reader -> reader.readText() }
    }
    @Before fun quiesceStartupBroadcasts() {
        check("All broadcast queues are idle" in shell("timeout 30 am wait-for-broadcast-idle"))
    }
    private fun withLedger(block: (PrivateMetadataStore, AndroidAlarms) -> Unit) {
        val prefs = privateMetadata(context, "alarm-ledger")
        val previous = prefs.all.toMap()
        val alarms = AndroidAlarms(context) { at }
        try {
            alarms.clear()
            check(prefs.edit().clear().commit()); alarms.reloadFromDisk()
            block(prefs, alarms)
        } finally {
            alarms.clear()
            val edit = prefs.edit().clear()
            previous.forEach { (key, value) -> edit.putString(key, value) }
            check(edit.commit()); alarms.reloadFromDisk(); alarms.reschedule()
        }
    }
    private fun dispatch(receiver: Class<out BroadcastReceiver>, code: Int? = null, generation: Long? = null) {
        val finished = CountDownLatch(1)
        val intent = Intent(context, receiver).setAction("app.porchlight.TEST_ALARM_DISPATCH")
        code?.let { intent.putExtra("code", it) }; generation?.let { intent.putExtra("generation", it) }
        // An explicit app-private action avoids impersonating a system-only TIME_SET broadcast.
        context.sendOrderedBroadcast(intent, null, object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) { finished.countDown() }
        }, Handler(Looper.getMainLooper()), Activity.RESULT_OK, null, null)
        assertTrue("Receiver must finish its asynchronous PendingResult", finished.await(30, TimeUnit.SECONDS))
    }
    // Only the live pending-alarm list: since API 31 dumpsys also keeps cancelled tags under "Removal history".
    private fun scheduled(ticket: AlarmTicket): Boolean {
        val lines = shell("dumpsys alarm").lines()
        val start = lines.indexOfFirst { it.trimEnd().endsWith("pending alarms:") }
        check(start >= 0) { "dumpsys alarm has no pending-alarm list" }
        return lines.drop(start + 1).takeWhile { it.isBlank() || it.startsWith("    ") }
            .any { "app.porchlight.REMINDER.${ticket.code}.${ticket.generation}" in it }
    }

    @Test fun wrapperReconcilePersistsSchedulesAndCancelsAcrossRecreation() = withLedger { _, alarms ->
        val item = WireJson.decodeFromString<ItemRead>("""{"id":"item","household_id":"h","title":"Fixture","notes":"","priority":0,"owner_id":"self","visibility":"household","kind":"todo","time_mode":"fixed","all_day":false,"buffer_before_min":0,"buffer_after_min":0,"missed_policy":"carry_over","version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z","assignees":[],"viewers":[],"reminders":[],"capabilities":["view"],"pending_derivation":false}""")
        val occurrence = WireJson.decodeFromString<OccurrenceRead>("""{"id":"occ","household_id":"h","item_id":"item","seq":1,"is_detached":false,"disposition":"open","status":"todo","board_position":"A","board_moved":false,"version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z"}""")
        val settings = WireJson.decodeFromString<SettingsRead>("""{"member_id":"self","version":1,"settings":{"today_sections":["today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}}""")
        val view = CacheView(items = listOf(item), occurrences = listOf(occurrence), settings = settings, sync = SyncState(1, at),
            reminders = listOf(ScheduledReminderRead("reminder", "h", "self", "occ", "item", "offset", at + 1.days, at)))
        alarms.reconcile(view)
        val ticket = alarms.tickets().single()
        assertTrue(scheduled(ticket))
        alarms.reloadFromDisk()
        val recreated = AndroidAlarms(context) { at }
        assertEquals(ticket, recreated.tickets().single())
        recreated.reconcile(view)
        assertEquals(ticket, recreated.tickets().single())
        recreated.reconcile(view.copy(occurrences = listOf(occurrence.copy(status = "done"))))
        assertFalse(recreated.tickets().single().active)
        assertFalse(scheduled(ticket))
        assertNull(recreated.pending(ticket.code, ticket.generation))
    }

    @Test fun manifestRescheduleReceiverSchedulesAndFinishesAsyncWork() {
        val services = AndroidAppServices.get(context)
        runBlocking(Dispatchers.IO) {
            services.trust.controller.ensureRestored()
            check(services.trust.controller.state.first { it != TrustState.Loading } == TrustState.Enrollment) {
                "Use an unenrolled disposable controller AVD"
            }
            services.data.cache.initialize()
            check(services.data.cache.sync.value == null) { "Controller fixture requires an empty read cache" }
        }
        withLedger { prefs, alarms ->
            val ledger = AlarmLedger()
            val ticket = ledger.schedule("receiver-fixture", at + 1.days, "item", "occ")
            check(prefs.edit().putString("table", ledger.encode()).commit()); alarms.reloadFromDisk()
            assertFalse(scheduled(ticket))
            dispatch(AlarmRescheduleReceiver::class.java)
            assertTrue(scheduled(ticket))
            alarms.reloadFromDisk()
            assertEquals(ticket, alarms.tickets().single())
        }
    }

    @Test fun manifestReminderReceiverRejectsStaleGenerationBeforeDelivery() {
        val services = AndroidAppServices.get(context)
        val cache = services.data.cache
        runBlocking(Dispatchers.IO) {
            services.trust.controller.ensureRestored()
            check(services.trust.controller.state.first { it != TrustState.Loading } == TrustState.Enrollment) {
                "Use an unenrolled disposable controller AVD"
            }
            cache.initialize()
            check(cache.sync.value == null) { "Controller fixture requires an empty read cache" }
        }
        val delivery = privateMetadata(context, "os-delivery")
        val previousDelivery = delivery.all.toMap()
        try { withLedger { _, alarms ->
            val now = kotlin.time.Clock.System.now()
            val due = now + 1.days
            val snapshot = WireJson.decodeFromString<SnapshotRead>("""{"notifications":[],"notification_prefs":{"member_id":"self","version":1,"prefs":{"reminders":true,"assignments":true,"dependents":true,"activity":true,"digest":false,"connection":true,"quiet_start":null,"quiet_end":null,"default_todo_offsets":[],"default_deadline_offsets":[],"default_appointment_offsets":[]}},"comments":[],"attachments":[],"ics_sources":[],"availability":[],"suggestions":[],"lists":[],"members":[],"items":[],"occurrences":[],"scheduled_reminders":[],"member_settings":{"member_id":"self","version":1,"settings":{"today_sections":["today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}},"cursor":1}""")
            val item = WireJson.decodeFromString<ItemRead>("""{"id":"item","household_id":"h","title":"Fixture","notes":"","priority":0,"owner_id":"self","visibility":"household","kind":"todo","time_mode":"fixed","all_day":false,"buffer_before_min":0,"buffer_after_min":0,"missed_policy":"carry_over","version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z","assignees":[],"viewers":[],"reminders":[],"capabilities":["view"],"pending_derivation":false}""")
            val occurrence = WireJson.decodeFromString<OccurrenceRead>("""{"id":"occ","household_id":"h","item_id":"item","seq":1,"is_detached":false,"disposition":"open","status":"todo","board_position":"A","board_moved":false,"version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z"}""")
            runBlocking(Dispatchers.IO) {
                // Household rows are owner-gated: bind a fixture owner as enrollment's binding would.
                cache.initialize(CacheOwner("fixture-ca", "self", "fixture-device"))
                cache.inSession { fence ->
                    cache.install(fence, snapshot.copy(items = listOf(item), occurrences = listOf(occurrence),
                        scheduledReminders = listOf(ScheduledReminderRead("stale-fixture", "h", "self", "occ", "item", "offset", due, now),
                            ScheduledReminderRead("admission-control", "h", "self", "occ", "item", "offset", due, now))))
                }
            }
            // The same authoritative fixture feeds the collector and the receiver's independent visibility guards.
            AndroidAlarms(context).reconcile(cache.view.value)
            val ticket = alarms.tickets().single { it.key == "stale-fixture" }
            // Positive control first: the receiver is admitted, so the stale-generation rejection below is not vacuous.
            val control = alarms.tickets().single { it.key == "admission-control" }
            assertTrue("Receiver must be admitted for the unenrolled fixture", (1..20).any {
                dispatch(ReminderReceiver::class.java, control.code, control.generation)
                alarms.reloadFromDisk()
                alarms.tickets().single { it.code == control.code }.delivered.also { done -> if (!done) Thread.sleep(500) }
            })
            dispatch(ReminderReceiver::class.java, ticket.code, ticket.generation - 1)
            alarms.reloadFromDisk()
            assertFalse("Stale generation must not deliver a visible, open occurrence", alarms.tickets().single { it.code == ticket.code }.delivered)
            assertNull(DeliveryInventory.decode(delivery.getString("table", null) ?: "[]").get("alarm:${ticket.code}"))
            dispatch(ReminderReceiver::class.java, ticket.code, ticket.generation)
            alarms.reloadFromDisk()
            assertTrue("Current generation must reach delivery with the identical item/occurrence guards", alarms.tickets().single { it.code == ticket.code }.delivered)
            assertNotNull(DeliveryInventory.decode(delivery.getString("table", null) ?: "[]").get("alarm:${ticket.code}"))
        } } finally {
            runBlocking(Dispatchers.IO) { cache.clear() }
            AndroidDelivery(context).clear()
            val edit = delivery.edit().clear()
            previousDelivery.forEach { (key, value) -> edit.putString(key, value) }
            check(edit.commit())
        }
    }
}
