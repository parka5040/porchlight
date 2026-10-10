package app.porchlight.shared.live

import app.porchlight.shared.data.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.withLock
import java.nio.file.*
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import app.porchlight.shared.transport.WireJson
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.milliseconds

private const val BUS_NOTICE = "Desktop tray/notifications unavailable. Household alerts remain in Inbox."
private const val NOTIFICATION_NOTICE = "Desktop notifications unavailable. Open Inbox for household alerts."

@Serializable private data class DesktopDeliveryState(val owner: String? = null, val records: List<DeliveryRecord> = emptyList())

class DesktopLive(private val data: HouseholdData, private val directory: Path, private val scope: CoroutineScope,
    private val now: () -> Instant, private val open: (String?, String?) -> Unit, quit: () -> Unit,
    val notice: MutableStateFlow<String?>, private val controller: app.porchlight.shared.trust.TrustController? = null,
    val trayActions: DesktopTrayActions = DesktopTrayActions(),
    private val sessionFactory: () -> DesktopSessionBus = { NativeDesktopSessionBus() },
    val trayAvailable: MutableStateFlow<Boolean?> = MutableStateFlow(null),
    private val alarmLoader: (DesktopAlarmStore) -> AlarmLedger = { it.load() },
    private val sleepBusFactory: () -> DesktopSleepBus = { NativeDesktopSleepBus() },
    val systemResumes: MutableStateFlow<Long> = MutableStateFlow(0L)) : AutoCloseable {
    @Volatile private var closed = false
    private var bus: DesktopBus? = null
    private var ledger = AlarmLedger()
    private var alarmsAvailable = true
    private var alarmFailureNotice: String? = null
    private var resumeMonitor: DesktopResumeMonitor? = null
    private fun setNotice(value: String?) {
        notice.value = listOfNotNull(alarmFailureNotice, value).distinct().joinToString(" ").ifEmpty { null }
    }
    private var inventory = DeliveryInventory()
    private var deliveryOwner: String? = null
    private val deliveryFile = directory.resolve("os-delivery.json")
    private val store = DesktopAlarmStore(directory)
    internal val alarmWaitingUntil = MutableStateFlow<Instant?>(null)
    private val alarmWake = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    private val busAvailable = MutableStateFlow(false)
    private val mutex = kotlinx.coroutines.sync.Mutex()
    private val initialized = CompletableDeferred<Unit>()
    private val jobs = mutableListOf<Job>()
    private val busRevision = MutableStateFlow(0L)
    private val quitAction = quit
    private fun connectBus() {
        bus = desktopBusOrNull {
            DesktopBus({ open(null, null) }, quitAction, { id, action -> scope.launch {
                initialized.await()
                if (controller == null) handle(id, action)
                else withBackgroundSession(controller) { handle(id, action) }
            } }, { id -> scope.launch {
                initialized.await(); mutex.lock()
                val key = try {
                    val record = inventory.get(id)
                    inventory.dismissed(id); persistDelivery()
                    record?.takeUnless { it.localAlarm }?.actionKey
                } finally { mutex.unlock() }
                if (key != null) try {
                    if (controller == null) dismissDeliveredNotification(data, id, key)
                    else withBackgroundSession(controller) { dismissDeliveredNotification(data, id, key) }
                } catch (e: CancellationException) { throw e }
                catch (_: Exception) { setNotice("Dismissal could not sync. Open Inbox and mark the alert read to retry.") }
            } }, sessionFactory(), trayActions, serverChanged = {
                scope.launch {
                    initialized.await(); mutex.lock()
                    try { synchronizeOwner(); busRevision.value++ } finally { mutex.unlock() }
                }
            }, degraded = { setNotice(BUS_NOTICE) }, connectionLost = {
                if (!closed) scope.launch {
                    initialized.await()
                    mutex.withLock {
                        if (!closed) {
                            runCatching { bus?.close() }; bus = null; busAvailable.value = false
                            synchronizeOwner(); busRevision.value++; setNotice(BUS_NOTICE)
                        }
                    }
                }
            })
        }
        busAvailable.value = bus != null
        if (bus == null) setNotice(BUS_NOTICE)
        synchronizeOwner()
    }
    private fun synchronizeOwner() {
        trayAvailable.value = bus?.trayAvailable == true
        val owner = bus?.notificationServerOwner
        if (owner != deliveryOwner) {
            inventory.records().filter { !it.dismissed }.forEach { inventory.put(it.copy(handle = 0, posted = false)) }
            deliveryOwner = owner; persistDelivery()
        }
        if (bus != null && bus?.trayAvailable == true && owner != null && notice.value in setOf(BUS_NOTICE, NOTIFICATION_NOTICE)) setNotice(null)
        else if (bus != null && bus?.trayAvailable == false) setNotice(BUS_NOTICE)
        else if (owner == null && bus != null) setNotice(NOTIFICATION_NOTICE)
    }
    init {
        data.localSnooze = snooze@ { occurrenceId ->
            initialized.await()
            if (!alarmsAvailable) return@snooze
            mutex.lock()
            try {
                ledger.snoozable(occurrenceId, now()).forEach { ticket ->
                    ledger.snooze(ticket.code, ticket.generation, now() + 15.minutes)
                    bus?.dismiss("alarm:${ticket.code}"); inventory.remove("alarm:${ticket.code}")
                }
                persist(); persistDelivery()
            } finally { mutex.unlock() }
        }
        jobs += scope.launch {
            Files.createDirectories(directory)
            data.cache.initialize()
            ledger = try { alarmLoader(store) } catch (_: Exception) {
                alarmsAvailable = false
                alarmFailureNotice = "Local alarms unavailable: inventory recovery failed. Open Inbox; check storage and restart Porchlight."
                setNotice(null)
                AlarmLedger() // never allocate/reuse request codes while the durable high-water is unknown
            }
            // One logind connection serves both: reconnect the stream, and tell screens (Today) that wall time jumped.
            resumeMonitor = DesktopResumeMonitor({ systemResumes.update { it + 1 }; data.live?.networkChanged() }, sleepBusFactory)
            if (store.recoveredDamage) setNotice("Local alarm inventory damaged and quarantined. Sync will rebuild future reminders.")
            val saved = if (Files.exists(deliveryFile)) runCatching { WireJson.decodeFromString<DesktopDeliveryState>(Files.readString(deliveryFile)) }.getOrElse {
                setNotice("Local delivery inventory damaged. Unread Inbox alerts will be re-delivered."); DesktopDeliveryState()
            } else DesktopDeliveryState()
            inventory = DeliveryInventory.decode(WireJson.encodeToString(saved.records))
            deliveryOwner = saved.owner
            connectBus()
            if (deliveryOwner == saved.owner && deliveryOwner != null) bus?.restoreHandles(inventory.records())
            initialized.complete(Unit)
            combine(data.cache.view, busRevision, data.live?.unreachableSince ?: MutableStateFlow<Instant?>(null)) { view, _, _ -> view }.dropWhile { it.sync == null }.collect { view ->
                mutex.lock()
                try { if (alarmsAvailable) { reconcileAlarms(ledger, view, now(), preserveOverdue = true); ledger.prune(now()); persist() } } finally { mutex.unlock() }
                mutex.lock()
                try {
                    inventory.toCancel(view).forEach { row ->
                        safely { bus?.let { it.dismiss(row.key); inventory.remove(row.key); persistDelivery() } }
                    }
                    inventory.serverToPost(view).forEach { row ->
                        inventory.put(inventory.prepareServer(row.id, row.version.toLong(), randomOperationKey())); persistDelivery()
                        safely { bus?.notify(row.id, row.summary.title, markupEscape(row.summary.body))?.let { inventory.posted(row.id, it); persistDelivery() } }
                    }
                    postPendingLocal(view)
                    safely { bus?.updateStatus(data.live?.unreachableSince?.value != null, view.notifications.count { it.readAt == null }) }
                } finally { mutex.unlock() }
            }
        }
        jobs += scope.launch {
            initialized.await()
            while (isActive) {
                mutex.lock()
                try {
                    val due = ledger.active().filter { it.fireAt <= now() }
                    due.forEach { ticket ->
                        val view = data.cache.view.value
                        val item = view.items.firstOrNull { it.id == ticket.itemId }
                        if (item != null && view.occurrences.any { it.id == ticket.occurrenceId && it.status in setOf("todo", "doing") }) {
                            val key = "alarm:${ticket.code}"
                            inventory.put(DeliveryRecord(key, ticket.generation, localAlarm = true, itemId = ticket.itemId, occurrenceId = ticket.occurrenceId, occurrenceVersion = view.occurrences.first { it.id == ticket.occurrenceId }.version, actionKey = randomOperationKey())); persistDelivery()
                            safely { bus?.notify(key, item.title, "Household reminder", listOf("default", "Open", "snooze", "Snooze 15 min", "complete", "Complete"))?.let { inventory.posted(key, it); persistDelivery() } }
                        }
                        ledger.fired(ticket.code, ticket.generation)
                    }
                    if (due.isNotEmpty()) persist()
                } finally { mutex.unlock() }
                val deadline = mutex.withLock { ledger.active().minOfOrNull { it.fireAt } }
                alarmWaitingUntil.value = deadline
                if (deadline == null) alarmWake.receive()
                else withTimeoutOrNull((deadline - now()).inWholeMilliseconds.coerceIn(1, 60_000)) { alarmWake.receive() }
            }
        }
        jobs += scope.launch {
            initialized.await()
            busAvailable.collectLatest { available ->
                if (!available) while (isActive) {
                    delay(15_000) // retry ONLY while no session bus exists, no healthy-owner polling
                    mutex.withLock { connectBus(); if (bus != null) busRevision.value++ }
                    if (bus != null) break
                }
            }
        }
        jobs.first().invokeOnCompletion { error ->
            if (!initialized.isCompleted) initialized.completeExceptionally(error ?: IllegalStateException("Desktop initialization failed"))
        }
    }

    private fun postPendingLocal(view: CacheView) {
        inventory.localToPost(view).forEach { row ->
            val item = view.items.first { it.id == row.itemId }
            safely { bus?.notify(row.key, item.title, "Household reminder", listOf("default", "Open", "snooze", "Snooze 15 min", "complete", "Complete"))?.let { inventory.posted(row.key, it); persistDelivery() } }
        }
    }
    suspend fun clearIdentity() {
        initialized.await(); mutex.lock()
        try {
            ledger.all().forEach { ledger.cancel(it.key) }; persist()
            inventory.records().forEach { row -> safely { bus?.let { it.dismiss(row.key); inventory.remove(row.key) } } }
            persistDelivery()
        } finally { mutex.unlock() }
    }
    private fun safely(block: () -> Unit) {
        try { block() } catch (_: Exception) { setNotice("Desktop notifications unavailable. Open Inbox for household alerts.") }
    }
    private fun persistDelivery() {
        val next = deliveryFile.resolveSibling("os-delivery.json.new")
        Files.writeString(next, WireJson.encodeToString(DesktopDeliveryState(deliveryOwner, inventory.records())))
        Files.move(next, deliveryFile, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
    }
    private fun persist() {
        if (!alarmsAvailable) { data.firedOccurrences.value = emptySet(); return }
        if (ledger.changes().isNotEmpty()) { store.save(ledger); alarmWake.trySend(Unit) }
        data.firedOccurrences.value = firedOccurrences(ledger.all())
    }
    private suspend fun handle(id: String, action: String) = data.cache.inSession {
        try {
            val record = inventory.get(id)?.takeIf { it.posted && !it.dismissed } ?: return@inSession
            if (id.startsWith("alarm:")) {
                mutex.lock()
                val ticket = try {
                    ledger.ticket(id.removePrefix("alarm:").toInt())?.takeIf { it.active && it.generation == record.version }?.also { if (action == "snooze") { ledger.snooze(it.key, now() + 15.minutes); bus?.dismiss(id); inventory.remove(id); persist(); persistDelivery() } }
                } finally { mutex.unlock() }
                if (ticket != null) when (action) {
                    "default" -> open(ticket.itemId, ticket.occurrenceId)
                    "complete" -> if (record.occurrenceVersion != null && record.actionKey != null) {
                        if (data.occurrences.complete(ticket.occurrenceId, record.occurrenceVersion, record.actionKey) is WriteResult.VersionConflict)
                            setNotice("Notification action could not complete. Open the item to review its current version.")
                    }
                }
            } else {
                val row = data.cache.view.value.notifications.firstOrNull { it.id == id } ?: return@inSession
                if (action == "read" && record.actionKey != null) data.notifications.read(id, record.actionKey) else if (action == "default") open(row.itemId, row.occurrenceId)
            }
        } catch (e: CancellationException) { throw e } catch (_: Exception) { setNotice("Notification action could not complete. Open Porchlight to retry.") }
    }
    override fun close() { closed = true; jobs.forEach { it.cancel() }; bus?.close(); resumeMonitor?.close() }
}
private fun markupEscape(text: String) = text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
