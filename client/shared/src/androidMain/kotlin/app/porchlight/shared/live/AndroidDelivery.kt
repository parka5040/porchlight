package app.porchlight.shared.live

import android.app.NotificationManager
import android.content.Context
import androidx.core.app.NotificationManagerCompat
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.data.randomOperationKey

/** Private, synchronously committed inventory; no notification bodies or session secrets are persisted here. */
class AndroidDelivery(private val context: Context) {
    private val prefs = app.porchlight.shared.settings.privateMetadata(context, "os-delivery")
    private val manager = context.getSystemService(NotificationManager::class.java)
    // Joint owner of this inventory and alarm-ledger; changed only after both retire successfully.
    internal fun owner(): app.porchlight.shared.data.CacheOwner? = synchronized(lock) {
        prefs.getString("cache-owner", null)?.let { runCatching {
            app.porchlight.shared.transport.WireJson.decodeFromString<app.porchlight.shared.data.CacheOwner>(it)
        }.getOrNull() }
    }
    internal fun saveOwner(owner: app.porchlight.shared.data.CacheOwner?) = synchronized(lock) {
        val edit = prefs.edit()
        if (owner == null) edit.remove("cache-owner") else edit.putString("cache-owner",
            app.porchlight.shared.transport.WireJson.encodeToString(app.porchlight.shared.data.CacheOwner.serializer(), owner))
        check(edit.commit())
    }
    private fun load() = prefs.getString("table", null)?.let(DeliveryInventory::decode) ?: DeliveryInventory()
    private fun save(inventory: DeliveryInventory) { check(prefs.edit().putString("table", inventory.encode()).commit()) }
    fun recordAlarm(ticket: AlarmTicket) = synchronized(lock) {
        val inventory = load()
        val key = "alarm:${ticket.code}"
        if (inventory.get(key)?.version != ticket.generation) {
            inventory.put(DeliveryRecord(key, ticket.generation, localAlarm = true, itemId = ticket.itemId, occurrenceId = ticket.occurrenceId))
            save(inventory)
        }
    }
    fun dismissed(key: String) = synchronized(lock) { val inventory = load(); inventory.dismissed(key); save(inventory) }
    fun cancelAlarm(code: Int) = synchronized(lock) {
        val inventory = load(); manager.cancel("alarm", code); inventory.remove("alarm:$code"); save(inventory)
    }
    fun clear() = synchronized(lock) {
        val old = pendingWithdrawals() + load().records()
        check(prefs.edit().putString("withdrawals", DeliveryInventory().also { inventory -> old.forEach(inventory::put) }.encode())
            .putString("table", DeliveryInventory().encode()).commit())
        drainWithdrawals() // failure retains only cancellation work, never an actionable delivery
        Unit
    }
    private fun pendingWithdrawals() = prefs.getString("withdrawals", null)?.let(DeliveryInventory::decode)?.records().orEmpty()
    private fun drainWithdrawals(): Boolean {
        val pending = DeliveryInventory().also { inventory -> pendingWithdrawals().forEach(inventory::put) }
        for (row in pending.records()) {
            try { cancel(row) } catch (_: Exception) { return false }
            pending.remove(row.key)
            check(prefs.edit().putString("withdrawals", pending.encode()).commit())
        }
        return true
    }
    private fun cancel(row: DeliveryRecord) {
        if (row.localAlarm) manager.cancel("alarm", row.key.removePrefix("alarm:").toInt()) else manager.cancel(row.key, 0)
    }
    fun reconcile(view: CacheView, current: () -> Boolean = { true }) = synchronized(lock) {
        if (!current() || !drainWithdrawals()) return@synchronized
        if (view.sync == null) return@synchronized // unavailable/rebuilding, not an authoritative empty household
        val inventory = load()
        inventory.toCancel(view).forEach { if (!current()) return@synchronized; cancel(it); inventory.remove(it.key); save(inventory) }
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return@synchronized
        inventory.serverToPost(view).forEach { row ->
            if (!current()) return@synchronized
            val record = inventory.prepareServer(row.id, row.version.toLong(), randomOperationKey())
            inventory.put(record); save(inventory) // write-ahead: crash/revoke after here retries by the same tag
            val notification = privateNotification(context, row.category.takeIf { it in deliveryChannels } ?: "activity", row.summary.title, row.summary.body)
                .setContentIntent(openAppIntent(context, row.itemId, row.occurrenceId))
                .setDeleteIntent(actionIntent(context, "dismiss", row.id, row.version, record.actionKey!!))
                .addAction(0, "Got it", actionIntent(context, "read", row.id, row.version, record.actionKey)).build()
            try { manager.notify(row.id, 0, notification); inventory.posted(row.id); save(inventory) }
            catch (_: SecurityException) { /* Pending inventory survives denial and is retried on permission recovery. */ }
        }
        inventory.localToPost(view).forEach { row ->
            if (!current()) return@synchronized
            val code = row.key.removePrefix("alarm:").toInt()
            val item = view.items.first { it.id == row.itemId }
            val occurrence = view.occurrences.first { it.id == row.occurrenceId }
            val notification = privateNotification(context, "reminders", item.title, "Household reminder")
                .setContentIntent(openAppIntent(context, item.id, occurrence.id))
                .setDeleteIntent(localAlarmActionIntent(context, "dismiss-local", code, row.version))
                .addAction(0, "Snooze 15 min", localAlarmActionIntent(context, "snooze", code, row.version))
                .addAction(0, "Complete", actionIntent(context, "complete", occurrence.id, occurrence.version)).build()
            try { manager.notify("alarm", code, notification); inventory.posted(row.key); save(inventory) }
            catch (_: SecurityException) { /* durable pending delivery */ }
        }
    }
    companion object { private val lock = Any() }
}
