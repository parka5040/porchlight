package app.porchlight.shared.live

import app.porchlight.shared.data.CacheView
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@Serializable data class AlarmTicket(val key: String, val code: Int, val generation: Long, val fireAt: Instant,
    val itemId: String, val occurrenceId: String, val active: Boolean = true, val snoozed: Boolean = false,
    val delivered: Boolean = false, val sourceFireAt: Instant = fireAt)
@Serializable private data class LedgerState(val nextCode: Int = 1, val tickets: List<AlarmTicket> = emptyList())
/** Codes are never reused, including after pruning. Only changed rows need durable writes. */
class AlarmLedger private constructor(var nextCode: Int, private val rows: MutableMap<String, AlarmTicket>) {
    private val codes = rows.values.associateByTo(mutableMapOf()) { it.code }
    private val dirty = linkedSetOf<String>()
    var revision: Long = 0; private set
    constructor() : this(1, linkedMapOf())
    private fun put(row: AlarmTicket) {
        if (rows[row.key] == row) return
        rows[row.key] = row; codes[row.code] = row; dirty += row.key; revision++
    }
    fun changes(): Map<String, AlarmTicket?> = dirty.associateWith { rows[it] }
    fun checkpoint() { dirty.clear() }
    fun all() = rows.values.toList()
    fun active() = rows.values.filter { it.active && !it.delivered }
    fun snoozable(occurrenceId: String, at: Instant) = rows.values.filter {
        it.active && it.occurrenceId == occurrenceId && (it.delivered || it.fireAt <= at)
    }
    fun ticket(code: Int) = codes[code]
    /** A split changes the parent, not the native code/generation or delivery state. */
    internal fun reparent(key: String, itemId: String): AlarmTicket =
        checkNotNull(rows[key]).copy(itemId = itemId).also(::put)
    fun schedule(key: String, at: Instant, itemId: String, occurrenceId: String): AlarmTicket {
        val old = rows[key]
        if (old != null && old.active && (old.fireAt == at || old.sourceFireAt == at)) return old
        check(nextCode < Int.MAX_VALUE) { "Alarm request codes exhausted" }
        val row = AlarmTicket(key, old?.code ?: nextCode++, (old?.generation ?: 0) + 1, at, itemId, occurrenceId)
        put(row); return row
    }
    fun snooze(key: String, at: Instant): AlarmTicket {
        val old = checkNotNull(rows[key])
        return old.copy(generation = old.generation + 1, fireAt = at, snoozed = true, delivered = false, active = true).also(::put)
    }
    /** Immutable Worker inputs: replay returns the same ticket; an old generation never overwrites a newer snooze. */
    fun snooze(code: Int, generation: Long, at: Instant): AlarmTicket? {
        val old = ticket(code)?.takeIf { it.active } ?: return null
        if (old.generation == generation + 1 && old.snoozed && old.fireAt == at) return old
        if (old.generation != generation) return null
        return snooze(old.key, at)
    }
    fun canFire(code: Int, generation: Long): Boolean = ticket(code)?.let { it.active && !it.delivered && it.generation == generation } ?: false
    fun fired(code: Int, generation: Long) { ticket(code)?.takeIf { canFire(code, generation) }?.let { put(it.copy(delivered = true, snoozed = false)) } }
    fun cancel(key: String) { rows[key]?.takeIf { it.active }?.let { put(it.copy(active = false, generation = it.generation + 1)) } }
    /** Identity retirement preserves allocation high-water, but no old reminder remains actionable. */
    fun retire(): List<AlarmTicket> = all().also { old ->
        old.forEach { rows.remove(it.key); codes.remove(it.code); dirty += it.key; revision++ }
    }
    fun prune(at: Instant): List<AlarmTicket> {
        val expired = rows.values.filter { (!it.active || it.delivered) && it.fireAt < at - 7.days }
        expired.forEach { rows.remove(it.key); codes.remove(it.code); dirty += it.key; revision++ }
        return expired
    }
    fun encode(): String = WireJson.encodeToString(LedgerState(nextCode, rows.values.toList()))
    companion object {
        fun restore(nextCode: Int, tickets: List<AlarmTicket>): AlarmLedger {
            require(nextCode > 0 && tickets.all { it.code in 1 until nextCode && it.generation > 0 })
            require(tickets.map { it.code }.distinct().size == tickets.size)
            require(tickets.map { it.key }.distinct().size == tickets.size)
            return AlarmLedger(nextCode, tickets.associateByTo(linkedMapOf()) { it.key })
        }
        fun decode(value: String): AlarmLedger {
            val state = WireJson.decodeFromString<LedgerState>(value)
            return restore(state.nextCode, state.tickets)
        }
    }
}
/** Server occurrence openness: only done and skipped close reminders. Missing occurrences are never eligible. */
internal fun String?.isOpenForReminders(): Boolean = this != null && this != "done" && this != "skipped"

fun reconcileAlarms(ledger: AlarmLedger, view: CacheView, at: Instant, preserveOverdue: Boolean = false) {
    val items = view.items.associateBy { it.id }
    val occurrences = view.occurrences.associateBy { it.id }
    val enabled = view.notificationPrefs?.prefs?.reminders != false
    val desired = view.reminders.filter { enabled && it.memberId == view.settings?.memberId && it.fireAt >= at && it.fireAt < at + 7.days
        && items[it.itemId]?.pendingDerivation == false && occurrences[it.occurrenceId]?.status.isOpenForReminders() }
    val keys = desired.mapTo(hashSetOf()) { it.id }
    val reminders = view.reminders.associateBy { it.id }
    for (ticket in ledger.all().filter { it.active }) {
        val source = reminders[ticket.key]
        val old = if (source != null && source.occurrenceId == ticket.occurrenceId && source.fireAt == ticket.sourceFireAt)
            ledger.reparent(ticket.key, source.itemId) else ticket
        val item = items[old.itemId]
        val occurrence = occurrences[old.occurrenceId]
        // Pending derivation preserves the known alarm, never schedules from possibly stale projections.
        if (!enabled || item == null || occurrence == null || !occurrence.status.isOpenForReminders() ||
            (!item.pendingDerivation && old.key !in keys && !old.snoozed &&
                !((old.delivered || (preserveOverdue && !old.delivered && old.fireAt <= at && at - old.fireAt < 7.days)) &&
                    source?.fireAt == old.sourceFireAt))) ledger.cancel(old.key)
    }
    desired.forEach { ledger.schedule(it.id, it.fireAt, it.itemId, it.occurrenceId) }
}
val deliveryChannels = listOf("reminders", "assignments", "dependents", "activity", "digest", "connection")

fun firedOccurrences(tickets: List<AlarmTicket>): Set<String> = tickets.filter { it.active && it.delivered }.mapTo(hashSetOf()) { it.occurrenceId }

fun notificationPublicTitle(channel: String): String = when (channel) {
    "connection" -> "Porchlight: household connection"
    else -> "Porchlight: 1 reminder"
}
