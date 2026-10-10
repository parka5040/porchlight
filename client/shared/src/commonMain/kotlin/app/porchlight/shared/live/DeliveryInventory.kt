package app.porchlight.shared.live

import app.porchlight.shared.data.CacheView
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString

/** Separate from disposable read cache. A pending record precedes OS delivery; posted follows it.
 * Crash between the two safely replaces the same OS tag. Local dismissal is a durable tombstone. */
@Serializable data class DeliveryRecord(val key: String, val version: Long, val handle: Long = 0,
    val posted: Boolean = false, val dismissed: Boolean = false, val localAlarm: Boolean = false,
    val itemId: String? = null, val occurrenceId: String? = null, val occurrenceVersion: Int? = null, val actionKey: String? = null)
/** The device controller gate exercises the same stored-key repository dispatch as this host-tested path. */
suspend fun dismissDeliveredNotification(data: app.porchlight.shared.data.HouseholdData, inventory: DeliveryInventory, id: String) =
    dismissDeliveredNotification(data, id, inventory.requirePostedAction(id).actionKey!!)
/** Production Worker receives the durable original key from its PendingIntent before the local tombstone. */
suspend fun dismissDeliveredNotification(data: app.porchlight.shared.data.HouseholdData, id: String, key: String) =
    data.notifications.read(id, key)

class DeliveryInventory private constructor(private val rows: MutableMap<String, DeliveryRecord>) {
    constructor() : this(linkedMapOf())
    fun records() = rows.values.toList()
    fun get(key: String) = rows[key]
    fun put(record: DeliveryRecord) { rows[record.key] = record }
    fun prepareServer(key: String, version: Long, operationKey: String): DeliveryRecord {
        val old = rows[key]?.takeIf { it.version == version }
        return DeliveryRecord(key, version, actionKey = old?.actionKey ?: operationKey).also(::put)
    }
    /** Shared by the real driver and host regression: never invent a key for a delivered action. */
    fun requirePostedAction(key: String): DeliveryRecord = checkNotNull(rows[key]).also {
        check(it.posted && !it.dismissed && it.actionKey != null) { "Device must have delivered the original alert" }
    }
    fun posted(key: String, handle: Long = 0) { rows[key]?.let { rows[key] = it.copy(posted = true, handle = handle) } }
    fun dismissed(key: String) { rows[key]?.let { rows[key] = it.copy(dismissed = true) } }
    fun remove(key: String) { rows.remove(key) }
    fun serverToPost(view: CacheView) = view.notifications.filter { n ->
        n.readAt == null && n.releasedAt != null && !n.summary.localAlarm &&
            (rows[n.id]?.let { it.version != n.version.toLong() || (!it.posted && !it.dismissed) } ?: true)
    }
    fun localToPost(view: CacheView) = records().filter { it.localAlarm && !it.posted && !it.dismissed && localValid(it, view) }
    fun toCancel(view: CacheView): List<DeliveryRecord> {
        val notifications = view.notifications.associateBy { it.id }
        return records().filter { if (it.localAlarm) !localValid(it, view) else notifications[it.key]?.readAt != null || it.key !in notifications }
    }
    private fun localValid(row: DeliveryRecord, view: CacheView) = view.notificationPrefs?.prefs?.reminders != false &&
        view.notifications.filter { it.summary.localAlarm && it.occurrenceId == row.occurrenceId }
            .maxByOrNull { it.createdAt }?.readAt == null &&
        view.items.any { it.id == row.itemId } && view.occurrences.any { it.id == row.occurrenceId && it.status in setOf("todo", "doing") }
    fun encode() = WireJson.encodeToString(records())
    companion object {
        fun decode(value: String): DeliveryInventory {
            val records = WireJson.decodeFromString<List<DeliveryRecord>>(value)
            require(records.map { it.key }.distinct().size == records.size)
            return DeliveryInventory(records.associateByTo(linkedMapOf()) { it.key })
        }
    }
}

/** Persisted random owner is cleared at identity-end, retained across same-identity re-login. */
fun actionIdentityOwner(identity: String, storedIdentity: String?, storedOwner: String?, newOwner: () -> String): String =
    storedOwner?.takeIf { storedIdentity == identity ||
        // Migration from device:member to stable device-only identity; identity-end clears owners first.
        (':' !in identity && storedIdentity?.substringBefore(':') == identity) } ?: newOwner()
