package app.porchlight.android

import android.Manifest
import android.app.AlarmManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.android.trust.ControllerOnly
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.FileInputStream
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

/** Run explicitly on a disposable API35/36 AVD. The controller sets BOTH platform states BEFORE instrumentation and
 * passes them as C4_EXACT_EXPECTED and C4_NOTIFICATIONS_EXPECTED, because exact-alarm revocation and `pm revoke` of
 * POST_NOTIFICATIONS kill the app; each test asserts it starts in exactly that state.
 *
 * API33+: notification enablement IS the POST_NOTIFICATIONS runtime grant. The POST_NOTIFICATION app-op follows the
 * grant and cannot disable a granted app, so denial is a real revocation. Denial -> recovery runs from either start:
 * a `denied` start is the cold-process denial the controller set, a `granted` start is revoked in-process with the
 * platform's test-only no-kill revocation (same OS state as `pm revoke`). Restores the starting grant and the private
 * inventories. No sleeps/network/enrollment or UI-thread runBlocking. */
@ControllerOnly
@RunWith(AndroidJUnit4::class)
class C4PlatformControllerTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private val at = Instant.parse("2035-01-01T00:00:00Z")
    private fun shell(command: String): String = instrumentation.uiAutomation.executeShellCommand(command).use { fd ->
        FileInputStream(fd.fileDescriptor).bufferedReader().use { it.readText() }
    }
    private fun mode(operation: String) = shell("cmd appops get ${context.packageName} $operation")
        .lineSequence().firstOrNull { it.contains("$operation:") }?.substringAfter("$operation:")?.trim()?.substringBefore(';')?.substringBefore(' ') ?: "default"
    private fun setMode(operation: String, value: String) { shell("cmd appops set ${context.packageName} $operation $value") }

    /** API35 sends BOOT_COMPLETED when instrumentation takes a freshly installed (stopped) app out of the stopped
     * state. It is enqueued when this process attaches, before the runner starts, and its receiver reschedules the
     * alarm ledger (load, then save) under AndroidAlarms' private lock, which these fixtures cannot take. Wait until
     * every queued broadcast, including goAsync receivers, has finished (with a deadline) before touching private
     * stores, so the app's own startup work never interleaves with fixture writes or snapshots. */
    @Before fun startupBroadcastsFinished() {
        val idle = shell("timeout $BROADCAST_IDLE_DEADLINE_SECONDS am wait-for-broadcast-idle")
        check("All broadcast queues are idle" in idle) { "Broadcasts still running after ${BROADCAST_IDLE_DEADLINE_SECONDS}s" }
        Log.i(TAG, "startup broadcasts finished; fixtures may touch private stores")
    }

    @Test fun exactDeniedFallbackReschedulesAcrossRecreatedAdapterAndRejectsStaleSnooze() {
        check(Build.VERSION.SDK_INT >= 31)
        val prefs = app.porchlight.shared.settings.privateMetadata(context, "alarm-ledger")
        val previous = prefs.all.toMap()
        val expected = InstrumentationRegistry.getArguments().getString("C4_EXACT_EXPECTED")
        check(expected in setOf("denied", "granted")) { "Set exact-alarm app-op before instrumentation; pass C4_EXACT_EXPECTED=denied or granted" }
        try {
            val ledger = AlarmLedger()
            val ticket = ledger.schedule("c4-controller", at + 15.minutes, "item", "occ")
            // Legacy-table migration seed; safe only because startupBroadcastsFinished() quiesced the app's receivers.
            check(prefs.edit().putString("table", ledger.encode()).commit())
            val manager = context.getSystemService(AlarmManager::class.java)
            assertEquals(expected == "granted", manager.canScheduleExactAlarms())
            val alarms = AndroidAlarms(context) { at }
            alarms.reschedule() // exact if granted, inexact if denied; neither may throw
            assertTrue(shell("dumpsys alarm").contains("app.porchlight.REMINDER.${ticket.code}.${ticket.generation}"))
            kotlinx.coroutines.runBlocking(kotlinx.coroutines.Dispatchers.IO) {
                rescheduleAfterBroadcast(context, Intent(Intent.ACTION_TIMEZONE_CHANGED))
            }
            assertEquals(ticket, AndroidAlarms(context) { at }.tickets().single())
            val first = alarms.snooze(ticket.code, ticket.generation, at + 30.minutes)
            assertNotNull(first)
            assertEquals(first, AndroidAlarms(context) { at }.snooze(ticket.code, ticket.generation, at + 30.minutes))
            assertNull(alarms.pending(ticket.code, ticket.generation))
            assertNotNull(alarms.pending(ticket.code, ticket.generation + 1))
        } finally {
            AndroidAlarms(context).clear()
            val edit = prefs.edit().clear()
            previous.forEach { (key, value) -> edit.putString(key, value) }
            check(edit.commit())
            AndroidAlarms(context).reloadFromDisk()
        }
    }

    @Test fun permissionRecoveryAndRestartDismissalUseCommittedOsInventory() {
        val expected = InstrumentationRegistry.getArguments().getString("C4_NOTIFICATIONS_EXPECTED")
        check(expected in setOf("denied", "granted")) {
            "Set POST_NOTIFICATIONS (pm revoke/grant) before instrumentation; pass C4_NOTIFICATIONS_EXPECTED=denied or granted"
        }
        val runtimePermission = Build.VERSION.SDK_INT >= 33
        check(runtimePermission || expected == "granted") { "API<33 has no POST_NOTIFICATIONS grant to revoke; pass granted" }
        val notifications = NotificationManagerCompat.from(context)
        // Fail closed when the device is not in the state the controller declared for this run.
        if (runtimePermission) assertEquals(expected == "granted", notificationPermissionGranted())
        assertEquals(expected == "granted", notifications.areNotificationsEnabled())
        val prefs = app.porchlight.shared.settings.privateMetadata(context, "os-delivery")
        val previous = prefs.getString("table", null)
        val notificationMode = mode("POST_NOTIFICATION")
        val row = Notification("c4-controller-notification", "comment", "activity", "item", "item", 1,
            NotificationSummary("New comment", "New comment", "A household member commented.", "comments", false), at, at, 1, releasedAt = at)
        try {
            check(prefs.edit().remove("table").commit())
            createNotificationChannels(context)
            if (runtimePermission) {
                if (notificationPermissionGranted()) revokeNotificationPermissionWithoutKill()
                assertFalse(notificationPermissionGranted())
            } else setMode("POST_NOTIFICATION", "deny") // legacy API<33 path, unchanged (no API30-32 AVD to verify)
            assertFalse(notifications.areNotificationsEnabled())
            AndroidDelivery(context).reconcile(CacheView(notifications = listOf(row)))
            assertTrue(DeliveryInventory.decode(prefs.getString("table", "[]")!!).records().isEmpty())
            shell("pm grant ${context.packageName} ${Manifest.permission.POST_NOTIFICATIONS}")
            if (runtimePermission) assertTrue(notificationPermissionGranted()) else setMode("POST_NOTIFICATION", "allow")
            assertTrue(notifications.areNotificationsEnabled())
            AndroidDelivery(context).reconcile(CacheView(notifications = listOf(row)))
            assertTrue(DeliveryInventory.decode(prefs.getString("table", "[]")!!).get(row.id)!!.posted)
            // A recreated adapter uses the persisted inventory, not a process-local previous snapshot.
            AndroidDelivery(context).reconcile(CacheView(notifications = listOf(row.copy(readAt = at))))
            assertNull(DeliveryInventory.decode(prefs.getString("table", "[]")!!).get(row.id))
            val ticket = AlarmTicket("c4-alarm", 9876, 1, at, "item", "occ")
            AndroidDelivery(context).recordAlarm(ticket)
            val item = WireJson.decodeFromString<ItemRead>("""{"id":"item","household_id":"h","title":"Private title","notes":"","priority":0,"owner_id":"self","visibility":"household","kind":"todo","time_mode":"fixed","all_day":false,"buffer_before_min":0,"buffer_after_min":0,"missed_policy":"carry_over","version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z","assignees":[],"viewers":[],"reminders":[],"capabilities":["view"],"pending_derivation":false}""")
            val occurrence = WireJson.decodeFromString<OccurrenceRead>("""{"id":"occ","household_id":"h","item_id":"item","seq":1,"is_detached":false,"disposition":"open","status":"todo","board_position":"A","board_moved":false,"version":1,"created_at":"2035-01-01T00:00:00Z","updated_at":"2035-01-01T00:00:00Z"}""")
            AndroidDelivery(context).reconcile(CacheView(items = listOf(item), occurrences = listOf(occurrence)))
            assertTrue(DeliveryInventory.decode(prefs.getString("table", "[]")!!).get("alarm:9876")!!.posted)
            AndroidDelivery(context).reconcile(CacheView(items = listOf(item), occurrences = listOf(occurrence.copy(status = "done"))))
            assertNull(DeliveryInventory.decode(prefs.getString("table", "[]")!!).get("alarm:9876"))
        } finally {
            AndroidDelivery(context).clear()
            check(prefs.edit().putString("table", previous).commit())
            if (runtimePermission) restoreNotificationPermission(expected == "granted")
            else setMode("POST_NOTIFICATION", notificationMode)
        }
    }

    private fun notificationPermissionGranted() =
        context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    /** PermissionManager#revokePostNotificationPermissionWithoutKillForTest (TestApi; `am instrument` grants test-API
     * access): the OS denial `pm revoke` produces, without killing this instrumentation process. Shell holds both
     * permissions; they are adopted only for this call. */
    private fun revokeNotificationPermissionWithoutKill() {
        val revoke = Class.forName("android.permission.PermissionManager").getMethod(
            "revokePostNotificationPermissionWithoutKillForTest", String::class.java, Int::class.javaPrimitiveType)
        val automation = instrumentation.uiAutomation
        automation.adoptShellPermissionIdentity(
            "android.permission.REVOKE_POST_NOTIFICATIONS_WITHOUT_KILL", "android.permission.REVOKE_RUNTIME_PERMISSIONS")
        try { revoke.invoke(context.getSystemService("permission"), context.packageName, android.os.Process.myUid() / PER_USER_RANGE) }
        finally { automation.dropShellPermissionIdentity() }
    }

    private fun restoreNotificationPermission(granted: Boolean) {
        if (granted && !notificationPermissionGranted()) shell("pm grant ${context.packageName} ${Manifest.permission.POST_NOTIFICATIONS}")
        if (!granted && notificationPermissionGranted()) revokeNotificationPermissionWithoutKill()
        check(notificationPermissionGranted() == granted) { "POST_NOTIFICATIONS starting grant was not restored" }
    }

    private companion object {
        const val TAG = "C4PlatformControllerTest"
        const val BROADCAST_IDLE_DEADLINE_SECONDS = 60
        const val PER_USER_RANGE = 100_000 // android.os.UserHandle.PER_USER_RANGE: user id = uid / range
    }
}
