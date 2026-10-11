package app.porchlight.shared.live

import android.app.*
import android.content.*
import android.os.Build
import androidx.core.app.NotificationCompat
import app.porchlight.shared.data.*
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.encodeToString
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.settings.PrivateStorageEntry
import app.porchlight.shared.settings.privateStorageBoundary
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlin.time.Clock
import kotlin.time.Duration.Companion.minutes

/** Ledger is not the disposable cache: Android stores it privately and commits before touching AlarmManager. */
class AndroidAlarms(private val context: Context, private val now: () -> kotlin.time.Instant = { Clock.System.now() }) {
    private val prefs = app.porchlight.shared.settings.privateMetadata(context, "alarm-ledger")
    private val manager = context.getSystemService(AlarmManager::class.java)
    private fun load(): AlarmLedger {
        val legacy = prefs.getString("table", null)
        val key = context.filesDir.absolutePath
        val cached = ledgers[key]
        if (cached != null && (legacy == null || legacy == legacyValues[key])) return cached
        val ledger = if (legacy != null) AlarmLedger.decode(legacy) else AlarmLedger.restore(prefs.getInt("nextCode", 1),
            prefs.all.filterKeys { it.startsWith("ticket:") }.values.map { WireJson.decodeFromString<AlarmTicket>(it as String) })
        ledgers[key] = ledger; legacyValues[key] = legacy
        return ledger
    }
    private fun save(ledger: AlarmLedger) {
        val legacy = prefs.contains("table")
        val changes = if (legacy) ledger.all().associateBy { it.key } else ledger.changes()
        if (changes.isEmpty() && !legacy) return
        val edit = prefs.edit().remove("table").putInt("nextCode", ledger.nextCode)
        if (legacy) prefs.all.keys.filter { it.startsWith("ticket:") }.forEach { edit.remove(it) }
        changes.forEach { (key, row) -> if (row == null) edit.remove("ticket:$key") else edit.putString("ticket:$key", WireJson.encodeToString(row)) }
        check(edit.commit()); ledger.checkpoint()
    }
    private fun prune(ledger: AlarmLedger) { ledger.prune(now()).forEach { manager.cancel(intent(it)) } }
    private fun intent(ticket: AlarmTicket) = PendingIntent.getBroadcast(context, ticket.code,
        Intent(context, ReminderReceiver::class.java).setAction("app.porchlight.REMINDER.${ticket.code}.${ticket.generation}").setData(android.net.Uri.parse("porchlight-local://alarm/${ticket.code}/${ticket.generation}")).putExtra("code", ticket.code).putExtra("generation", ticket.generation),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    private fun schedule(ticket: AlarmTicket) {
        val pending = intent(ticket)
        val at = ticket.fireAt.toEpochMilliseconds()
        if (Build.VERSION.SDK_INT < 31 || manager.canScheduleExactAlarms()) {
            try { manager.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending); return }
            catch (_: SecurityException) { /* Permission revoked between check and syscall; inexact is still useful. */ }
        }
        manager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pending)
    }
    fun reconcile(view: CacheView, current: () -> Boolean = { true }) = synchronized(lock) {
        if (!current() || !drainWithdrawals()) return@synchronized
        if (view.sync == null) return@synchronized // unavailable/rebuilding, not an authoritative empty household
        val ledger = load(); val old = ledger.all().filter { it.active }
        reconcileAndroidAlarms(ledger, view, now()); prune(ledger); save(ledger)
        old.filter { ticket -> ledger.active().none { it.code == ticket.code && it.generation == ticket.generation } }.forEach { if (!current()) return@synchronized; manager.cancel(intent(it)) }
        // Parent-only split updates retain the existing PendingIntent and its delivery deadline.
        ledger.active().filter { ticket -> old.none { it.code == ticket.code && it.generation == ticket.generation } }.forEach { if (!current()) return@synchronized; schedule(it) }
    }
    fun reschedule(current: () -> Boolean = { true }) = synchronized(lock) {
        if (!current() || !drainWithdrawals()) return@synchronized
        val ledger = load(); prune(ledger); save(ledger)
        ledger.active().forEach { if (!current()) return@synchronized; schedule(it) }
    }
    fun pending(code: Int, generation: Long): AlarmTicket? = synchronized(lock) {
        load().let { ledger -> ledger.ticket(code)?.takeIf { ledger.canFire(code, generation) } }
    }
    fun fire(code: Int, generation: Long): AlarmTicket? = synchronized(lock) {
        val ledger = load()
        if (!ledger.canFire(code, generation)) return@synchronized null
        ledger.ticket(code).also { ledger.fired(code, generation); save(ledger) }
    }
    fun clear() = synchronized(lock) {
        val ledger = load()
        val pending = (pendingWithdrawals() + ledger.all()).distinctBy { it.code to it.generation }
        check(prefs.edit().putString("withdrawals", WireJson.encodeToString(pending)).commit())
        ledger.retire(); save(ledger)
        drainWithdrawals()
        Unit
    }
    private fun pendingWithdrawals(): List<AlarmTicket> = prefs.getString("withdrawals", null)?.let { WireJson.decodeFromString(it) } ?: emptyList()
    private fun drainWithdrawals(): Boolean {
        val pending = pendingWithdrawals().toMutableList()
        for (ticket in pending.toList()) {
            try { manager.cancel(intent(ticket)) } catch (_: Exception) { return false }
            pending.remove(ticket)
            check(prefs.edit().putString("withdrawals", WireJson.encodeToString(pending)).commit())
        }
        return true
    }
    /** Controller fixtures may restore preference files in-process; normal runtime never needs to re-decode them. */
    fun reloadFromDisk() = synchronized(lock) { ledgers.remove(context.filesDir.absolutePath); legacyValues.remove(context.filesDir.absolutePath) }
    fun tickets(): List<AlarmTicket> = synchronized(lock) { load().all() }
    fun snooze(code: Int, generation: Long, at: kotlin.time.Instant): AlarmTicket? = synchronized(lock) {
        val ledger = load(); val old = ledger.ticket(code) ?: return@synchronized null
        val ticket = ledger.snooze(code, generation, at) ?: return@synchronized null
        save(ledger); manager.cancel(intent(old)); schedule(ticket)
        AndroidDelivery(context).cancelAlarm(code)
        ticket
    }
    fun snoozeOccurrence(occurrenceId: String) = synchronized(lock) {
        load().snoozable(occurrenceId, now()).forEach { ticket ->
            enqueueLocalSnooze(context, ticket, now() + 15.minutes)
        }
    }
    companion object {
        private val lock = Any()
        private val ledgers = mutableMapOf<String, AlarmLedger>()
        private val legacyValues = mutableMapOf<String, String?>()
    }
}
/** Lock-screen privacy is per notification ([privateNotification]): channel lockscreenVisibility is system/user-owned,
 * and Android stores app-created channels as VISIBILITY_NO_OVERRIDE whatever the app sets. */
fun createNotificationChannels(context: Context) {
    val manager = context.getSystemService(NotificationManager::class.java)
    val names = listOf("Reminders", "Assignments", "Dependents' appointments", "Household activity", "Digest", "Connection")
    manager.createNotificationChannels(deliveryChannels.zip(names).map { (id, name) ->
        NotificationChannel(id, name, if (id == "connection") NotificationManager.IMPORTANCE_MIN else NotificationManager.IMPORTANCE_DEFAULT)
    })
}
fun openAppIntent(context: Context, itemId: String? = null, occurrenceId: String? = null): PendingIntent {
    val intent = context.packageManager.getLaunchIntentForPackage(context.packageName)!!
        .putExtra("item_id", itemId).putExtra("occurrence_id", occurrenceId)
    intent.data = android.net.Uri.parse("porchlight-local://open/${itemId.orEmpty()}/${occurrenceId.orEmpty()}")
    return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
}
fun privateNotification(context: Context, channel: String, title: String, body: String): NotificationCompat.Builder {
    val public = NotificationCompat.Builder(context, channel).setSmallIcon(app.porchlight.shared.R.drawable.porchlight_notification)
        .setContentTitle(notificationPublicTitle(channel)).setVisibility(NotificationCompat.VISIBILITY_PUBLIC).build()
    return NotificationCompat.Builder(context, channel).setSmallIcon(app.porchlight.shared.R.drawable.porchlight_notification)
        .setContentTitle(title).setContentText(body).setStyle(NotificationCompat.BigTextStyle().bigText(body))
        .setVisibility(NotificationCompat.VISIBILITY_PRIVATE).setPublicVersion(public).setAutoCancel(true)
}
class ReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try { privateStorageBoundary(context).attempt(PrivateStorageEntry.Receiver) {
                val code = intent.getIntExtra("code", -1)
                val generation = intent.getLongExtra("generation", -1)
                val services = AndroidAppServices.get(context)
                services.data.cache.initialize() // disk only: due delivery never needs a network request or PIN
                services.trust.controller.ensureRestored()
                services.trust.controller.state.first { it != app.porchlight.shared.trust.TrustState.Loading }
                services.liveRuntime.withCurrent { view ->
                    val ticket = AndroidAlarms(context).pending(code, generation) ?: return@withCurrent
                    if (view.items.none { it.id == ticket.itemId } || view.occurrences.none { it.id == ticket.occurrenceId && it.status.isOpenForReminders() }) return@withCurrent
                    notificationActionOwner(context, services.trust.controller)
                    createNotificationChannels(context)
                    AndroidDelivery(context).recordAlarm(ticket) // denied delivery is durable and recoverable
                    AndroidAlarms(context).fire(code, ticket.generation)
                    services.data.firedOccurrences.value = firedOccurrences(AndroidAlarms(context).tickets())
                    AndroidDelivery(context).reconcile(view, services.liveRuntime::current)
                }
            }.onFailure { runCatching { enqueueSafetyNet(context) } } } finally { pending?.finish() }
        }
    }
}
class AlarmRescheduleReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            runAlarmReschedule(pending) {
                privateStorageBoundary(context).attempt(PrivateStorageEntry.Receiver) { rescheduleAfterBroadcast(context, intent) }
                    .onFailure { runCatching { enqueueSafetyNet(context) } }
            }
        }
    }
}

/** Shared by receiver dispatch and host tests (goAsync is null outside framework dispatch). */
internal suspend fun runAlarmReschedule(pending: BroadcastReceiver.PendingResult?, work: suspend () -> Unit) {
    try { work() } finally { pending?.finish() }
}

/** Awaitable production reschedule body, also used by the instrumentation fixture. */
suspend fun rescheduleAfterBroadcast(context: Context, intent: Intent) {
    if (intent.action in setOf(Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED)) {
        val services = AndroidAppServices.get(context)
        if (services.settings.settings.first().stayConnected) updateForegroundConnection(context, true)
    }
    val services = AndroidAppServices.get(context)
    services.data.cache.initialize()
    recoverLivePermissions(context, services) // advance the window on boot/time/zone changes as well as permission changes
}

/** The cache collector can reconcile before a cold-process ReminderReceiver marks its ticket fired. */
internal fun reconcileAndroidAlarms(ledger: AlarmLedger, view: CacheView, at: kotlin.time.Instant) =
    reconcileAlarms(ledger, view, at, preserveOverdue = true)
