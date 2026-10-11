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

@Serializable private data class DesktopWithdrawal(val owner: String, val handle: Long)
@Serializable private data class DesktopDeliveryState(val owner: String? = null, val records: List<DeliveryRecord> = emptyList(),
    val withdrawals: List<DesktopWithdrawal> = emptyList(), val identity: CacheOwner? = null)
private const val MAX_NATIVE_UNITS = 16

internal fun writeDesktopDelivery(file: Path, text: String) {
    val next = file.resolveSibling("os-delivery.json.new")
    Files.writeString(next, text)
    Files.move(next, file, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
}

class DesktopLive(private val data: HouseholdData, private val directory: Path, private val scope: CoroutineScope,
    private val now: () -> Instant, private val open: (String?, String?) -> Unit, quit: () -> Unit,
    val notice: MutableStateFlow<String?>, private val controller: app.porchlight.shared.trust.TrustController? = null,
    val trayActions: DesktopTrayActions = DesktopTrayActions(),
    private val sessionFactory: () -> DesktopSessionBus = { NativeDesktopSessionBus() },
    val trayAvailable: MutableStateFlow<Boolean?> = MutableStateFlow(null),
    private val alarmLoader: (DesktopAlarmStore) -> AlarmLedger = { it.load() },
    private val sleepBusFactory: () -> DesktopSleepBus = { NativeDesktopSleepBus() },
    val systemResumes: MutableStateFlow<Long> = MutableStateFlow(0L),
    private val onAlarmWait: (Instant?) -> Unit = {},
    private val beforeReconcile: suspend () -> Unit = {},
    private val deliveryWriter: (Path, String) -> Unit = ::writeDesktopDelivery,
    private val monotonicNanos: () -> Long = System::nanoTime,
    private val notifyRetryDelay: suspend (Long) -> Unit = { delay(it) }) : AutoCloseable {
    @Volatile private var closed = false
    private var bus: DesktopBus? = null
    private var ledger = AlarmLedger()
    private var alarmsAvailable = true
    private var alarmFailureNotice: String? = null
    private var resumeMonitor: DesktopResumeMonitor? = null
    private var busFailureNotice: String? = null
    private var detailNotice: String? = null
    private var recoveryNotice: String? = null
    private fun publishNotice() {
        notice.value = listOfNotNull(alarmFailureNotice, busFailureNotice, detailNotice, recoveryNotice).distinct().joinToString(" ").ifEmpty { null }
    }
    private fun setNotice(value: String?) { detailNotice = value; publishNotice() }
    private fun setBusNotice(value: String?) { busFailureNotice = value; publishNotice() }
    private var inventory = DeliveryInventory()
    private var deliveryOwner: String? = null
    private var inventoryOwner: CacheOwner? = null
    private val withdrawals = linkedSetOf<DesktopWithdrawal>()
    private val deliveryFile = directory.resolve("os-delivery.json")
    private val store = DesktopAlarmStore(directory)
    private val alarmWake = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    private val busAvailable = MutableStateFlow(false)
    private val mutex = kotlinx.coroutines.sync.Mutex()
    private val identity = LiveIdentityFence(controller, mutex)
    private val initialized = CompletableDeferred<Unit>()
    private val jobs = java.util.concurrent.CopyOnWriteArrayList<Job>()
    private val busRevision = MutableStateFlow(0L)
    private val quitAction = quit
    private data class PendingNotify(val target: DesktopBus, val row: DeliveryRecord, val epoch: Long,
        val owner: String?, val busOwner: String?, val call: DesktopNotifyCall?,
        val reply: java.util.concurrent.CompletableFuture<Long>)
    private val pendingNotifies = java.util.concurrent.ConcurrentHashMap.newKeySet<PendingNotify>()
    // Reply ownership outlives stream/session jobs and is drained before this scope is cancelled.
    private val completions = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var unsentRollbackNeeded = false
    private var notifyRetryAfter = 0L
    private var notifyRetryMillis = 1_000L
    private var observedNotificationOwner: String? = null
    private var notifyRetryJob: Job? = null
    private fun resetNotifyRetry() {
        notifyRetryJob?.cancel(); notifyRetryJob = null
        notifyRetryAfter = 0L; notifyRetryMillis = 1_000L
    }
    private fun retryNotifyLater() {
        setBusNotice(NOTIFICATION_NOTICE)
        val wait = notifyRetryMillis
        notifyRetryMillis = (wait * 2).coerceAtMost(300_000L)
        notifyRetryAfter = monotonicNanos() + wait * 1_000_000L
        notifyRetryJob?.cancel()
        notifyRetryJob = scope.launch { notifyRetryDelay(wait); busRevision.update { it + 1 }; alarmWake.trySend(Unit) }
    }
    private fun connectBus() {
        if (closed) return
        bus = desktopBusOrNull {
            DesktopBus({ open(null, null) }, quitAction, { id, action -> scope.launch {
                initialized.await()
                if (controller == null) handle(id, action)
                else withBackgroundSession(controller) { handle(id, action) }
            } }, { id -> scope.launch {
                initialized.await()
                val epoch = identity.epoch()
                var key: String? = null
                identity.run(epoch) {
                    val record = inventory.get(id)
                    inventory.dismissed(id); persistDelivery()
                    key = record?.takeUnless { it.localAlarm }?.actionKey
                }
                if (key != null) try {
                    if (controller == null) dismissDeliveredNotification(data, id, checkNotNull(key))
                    else withBackgroundSession(controller) { dismissDeliveredNotification(data, id, checkNotNull(key)) }
                } catch (e: CancellationException) { throw e }
                catch (_: Exception) { setNotice("Dismissal could not sync. Open Inbox and mark the alert read to retry.") }
            } }, sessionFactory(), trayActions, serverChanged = {
                scope.launch {
                    initialized.await(); mutex.lock()
                    try { if (!closed) { synchronizeOwner(); busRevision.value++ } } finally { mutex.unlock() }
                }
            }, degraded = { setBusNotice(BUS_NOTICE) }, connectionLost = {
                if (!closed) scope.launch {
                    initialized.await()
                    mutex.withLock {
                        if (!closed) {
                            runCatching { bus?.close() }; bus = null; busAvailable.value = false
                            synchronizeOwner(); busRevision.value++; setBusNotice(BUS_NOTICE)
                        }
                    }
                }
            })
        }
        busAvailable.value = bus != null
        if (bus == null) setBusNotice(BUS_NOTICE)
        synchronizeOwner()
    }
    private fun synchronizeOwner() {
        trayAvailable.value = bus?.trayAvailable == true
        val owner = bus?.notificationServerOwner
        if (owner != observedNotificationOwner) { observedNotificationOwner = owner; resetNotifyRetry() }
        // A disconnected bus is not proof that the unique notification owner disappeared.
        // Retain handles for owner-bound withdrawal; a different known owner may safely replay live rows.
        if (owner == null) {
            val posted = inventory.records().filter { !it.dismissed && it.posted }
            posted.forEach { inventory.put(it.copy(posted = false)) }
            if (posted.isNotEmpty()) persistDelivery()
        }
        if (owner != null && owner != deliveryOwner) {
            inventory.records().filter { !it.dismissed }.forEach { inventory.put(it.copy(handle = 0, posted = false, inFlight = false)) }
            deliveryOwner = owner; persistDelivery()
        }
        if (owner != null && owner == deliveryOwner) bus?.restoreHandles(inventory.records())
        if (bus != null && bus?.trayAvailable == true && owner != null && busFailureNotice != null) setBusNotice(null)
        else if (bus != null && bus?.trayAvailable == false) setBusNotice(BUS_NOTICE)
        else if (owner == null && bus != null) setBusNotice(NOTIFICATION_NOTICE)
    }
    init {
        data.localSnooze = snooze@ { occurrenceId ->
            val epoch = identity.epoch()
            initialized.await()
            if (!alarmsAvailable) return@snooze
            identity.run(epoch) {
                for (ticket in ledger.snoozable(occurrenceId, now()).take(MAX_NATIVE_UNITS)) {
                    if (!identity.current(epoch)) break
                    ledger.snooze(ticket.code, ticket.generation, now() + 15.minutes); persist()
                    inventory.get("alarm:${ticket.code}")?.let(::retireDelivery); persistDelivery()
                    if (!drainWithdrawal()) break
                }
            }
        }
        jobs += scope.launch {
            launch { data.cache.storageNotice.collect { if (it != null || detailNotice == CACHE_STORAGE_NOTICE) setNotice(it) } }
            while (isActive) {
                try { initializeInventory(); break }
                catch (e: CancellationException) { throw e }
                catch (_: Exception) {
                    mutex.withLock {
                        if (!initialized.isCompleted) {
                            recoveryNotice = "Local delivery storage unavailable. Retrying inventory recovery."
                            publishNotice()
                        }
                    }
                    delay(1000)
                }
            }
            combine(data.cache.view, busRevision, controller?.liveAdmission ?: MutableStateFlow<Long?>(0),
                data.live?.unreachableSince ?: MutableStateFlow<Instant?>(null)) { _, _, _, _ -> Unit }.collect {
                val epoch = identity.epoch() // admission BEFORE reading any cache projection
                beforeReconcile()
                identity.run(epoch) { reconcile(epoch) }
            }
        }
        jobs += scope.launch {
            initialized.await()
            while (isActive) {
                val epoch = identity.epoch()
                beforeReconcile()
                identity.run(epoch) { reconcile(epoch) }
                val deadline = mutex.withLock {
                    if (data.cache.sync.value == null || !identity.current(epoch) || bus?.notificationServerOwner == null) null
                    else ledger.active().minOfOrNull { it.fireAt }
                }
                onAlarmWait(deadline)
                withTimeoutOrNull(deadline?.let { (it - now()).inWholeMilliseconds.coerceIn(1, 60_000) } ?: 60_000) { alarmWake.receive() }
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
    }
    private suspend fun initializeInventory() {
        if (initialized.isCompleted) return
        data.cache.awaitInitialized()
        mutex.withLock {
            if (initialized.isCompleted) return
            Files.createDirectories(directory)
            ledger = try { alarmLoader(store).also { alarmsAvailable = true; alarmFailureNotice = null; publishNotice() } } catch (_: Exception) {
                alarmsAvailable = false
                alarmFailureNotice = "Local alarms unavailable: inventory recovery failed. Open Inbox; check storage and restart Porchlight."
                publishNotice()
                AlarmLedger() // never allocate/reuse request codes while the durable high-water is unknown
            }
            if (store.recoveredDamage) setNotice("Local alarm inventory damaged and quarantined. Sync will rebuild future reminders.")
            val saved = if (Files.exists(deliveryFile)) {
                val text = Files.readString(deliveryFile) // unreadable storage must retry, not overwrite an unknown inventory
                runCatching { WireJson.decodeFromString<DesktopDeliveryState>(text) }.getOrElse {
                    setNotice("Local delivery inventory damaged. Unread Inbox alerts will be re-delivered."); DesktopDeliveryState()
                }
            } else DesktopDeliveryState()
            inventory = DeliveryInventory.decode(WireJson.encodeToString(saved.records))
            deliveryOwner = saved.owner
            inventoryOwner = saved.identity
            withdrawals.clear(); withdrawals += saved.withdrawals
            runCatching { bus?.close() }; bus = null
            connectBus()
            if (deliveryOwner == saved.owner && deliveryOwner != null) bus?.restoreHandles(inventory.records())
            // One logind connection serves both stream reconnection and wall-time changes.
            resumeMonitor = DesktopResumeMonitor({ systemResumes.update { it + 1 }; data.live?.networkChanged(); alarmWake.trySend(Unit) }, sleepBusFactory)
            recoveryNotice = null; publishNotice()
            initialized.complete(Unit) // failures are retryable; never publish an exceptionally completed barrier
        }
    }

    /** Runtime lock is held, never the global commit lock. Stop after one native failure or a bounded pass. */
    private fun reconcile(epoch: Long) {
        val view = data.cache.view.value
        var units = 0
        if (closed) return
        fun admitted() = !closed && identity.current(epoch) && units++ < MAX_NATIVE_UNITS
        fun native(work: () -> Unit): Boolean = try { work(); true } catch (_: Exception) { setBusNotice(NOTIFICATION_NOTICE); false }
        if (unsentRollbackNeeded && !native { persistDelivery(); unsentRollbackNeeded = false }) {
            retryNotifyLater(); return
        }
        for (row in withdrawals.toList()) {
            if (!admitted()) return
            val target = bus ?: return
            if (!native { target.withdraw(row.owner, row.handle) }) return
            withdrawals.remove(row); persistDelivery()
        }
        if (view.sync == null) return
        if (alarmsAvailable) { reconcileAlarms(ledger, view, now(), preserveOverdue = true); ledger.prune(now()); persist() }
        for (row in inventory.toCancel(view)) {
            if (!admitted()) return
            retireDelivery(row); persistDelivery()
            if (!drainWithdrawal()) return
        }
        // Owner changes wake reconciliation. An absent daemon is not an attempted post: neither prepare rows
        // nor fire overdue reminders into delivery storage, and never start a polling rejection/backoff loop.
        if (bus?.notificationServerOwner == null) {
            if (admitted()) native { bus?.updateStatus(data.live?.unreachableSince?.value != null, view.notifications.count { it.readAt == null }) }
            return
        }
        for (row in inventory.serverToPost(view)) {
            if (monotonicNanos() < notifyRetryAfter) break
            if (!admitted()) { alarmWake.trySend(Unit); return }
            val target = bus ?: return
            // Changed content may supersede an ambiguous version, but must not reuse its replacement handle:
            // withdrawing the old late reply must never close the new version's notification.
            inventory.get(row.id)?.takeIf { it.inFlight }?.let(::retireDelivery)
            val previousHandle = inventory.get(row.id)?.handle ?: 0
            inventory.put(inventory.prepareServer(row.id, row.version.toLong(), randomOperationKey()).copy(handle = previousHandle)); persistDelivery()
            if (!native { postDelivery(target, checkNotNull(inventory.get(row.id)), epoch, row.summary.title, markupEscape(row.summary.body)) }) return
        }
        for (ticket in ledger.active().filter { it.fireAt <= now() }) {
            if (!admitted()) { alarmWake.trySend(Unit); return }
            val item = view.items.firstOrNull { it.id == ticket.itemId }
            if (item != null && view.occurrences.any { it.id == ticket.occurrenceId && it.status.isOpenForReminders() }) {
                val key = "alarm:${ticket.code}"
                inventory.put(DeliveryRecord(key, ticket.generation, localAlarm = true, itemId = ticket.itemId,
                    occurrenceId = ticket.occurrenceId, occurrenceVersion = view.occurrences.first { it.id == ticket.occurrenceId }.version,
                    actionKey = randomOperationKey())); persistDelivery()
            }
            ledger.fired(ticket.code, ticket.generation); persist()
        }
        for (row in inventory.localToPost(view)) {
            if (monotonicNanos() < notifyRetryAfter) break
            if (!admitted()) { alarmWake.trySend(Unit); return }
            val target = bus ?: return
            val item = view.items.first { it.id == row.itemId }
            // The pending record above was committed before calling the daemon.
            if (!native { postDelivery(target, row, epoch, item.title, "Household reminder",
                    listOf("default", "Open", "snooze", "Snooze 15 min", "complete", "Complete")) }) return
        }
        if (admitted()) native { bus?.updateStatus(data.live?.unreachableSince?.value != null, view.notifications.count { it.readAt == null }) }
    }
    private fun postDelivery(target: DesktopBus, row: DeliveryRecord, epoch: Long, title: String, body: String,
        actions: List<String> = listOf("default", "Open", "read", "Got it")) {
        // Ambiguous submissions survive restart as non-repostable, including a handle-0 call whose reply is late.
        inventory.put(row.copy(inFlight = true))
        try { persistDelivery() } catch (e: Exception) {
            // No call was submitted. Restore memory even if undoing a possibly committed marker also fails;
            // the retry must commit that rollback before it may start any native work.
            inventory.put(row)
            unsentRollbackNeeded = true
            try { persistDelivery(); unsentRollbackNeeded = false } catch (undo: Exception) { e.addSuppressed(undo) }
            if (!closed) retryNotifyLater()
            throw e
        }
        val owner = deliveryOwner
        val busOwner = target.notificationServerOwner
        val submitted = runCatching { target.beginNotify(row.key, title, body, actions) }
        val call = submitted.getOrNull()
        val reply = call?.reply ?: java.util.concurrent.CompletableFuture.failedFuture(checkNotNull(submitted.exceptionOrNull()))
        val pending = PendingNotify(target, row, epoch, owner, busOwner, call, reply)
        pendingNotifies += pending
        // Fast replies (and synchronous test adapters) commit in this pass. Native waits never hold the runtime lock.
        if (!runCatching { finishNotify(pending) }.getOrDefault(false)) {
            reply.whenComplete { _, _ -> completions.launch {
                while (isActive && pending in pendingNotifies) {
                    try { mutex.withLock { finishNotify(pending) }; break }
                    catch (e: CancellationException) { throw e }
                    catch (_: Exception) { delay(1000) } // retain even an already-known reply until persistence succeeds
                }
            } }
            completions.launch {
                delay(2_000)
                mutex.withLock { if (!closed && pending in pendingNotifies && !reply.isDone) setBusNotice(NOTIFICATION_NOTICE) }
            }
        }
        if (reply.isCompletedExceptionally) throw java.io.IOException("Notification attempt failed")
    }
    private fun finishNotify(pending: PendingNotify): Boolean {
        if (pending !in pendingNotifies) return true
        if (!pending.reply.isDone) return false
        val result = runCatching { pending.reply.join() }
        val failure = result.exceptionOrNull()?.let { if (it is java.util.concurrent.CompletionException) it.cause else it }
        if (failure == null) completeDelivery(pending.target, pending.row, pending.epoch, checkNotNull(pending.call), result.getOrThrow())
        else if (failure is DesktopNotifyRejected) rejectDelivery(pending.target, pending.row, pending.epoch, pending.owner, pending.busOwner)
        else setBusNotice(NOTIFICATION_NOTICE) // connection loss after submission is still ambiguous, never a resend
        pendingNotifies.remove(pending)
        return true
    }
    private fun rejectDelivery(target: DesktopBus, row: DeliveryRecord, epoch: Long, owner: String?, busOwner: String?) {
        val current = inventory.get(row.key)
        if (identity.current(epoch) && deliveryOwner == owner && target.notificationServerOwner == busOwner && current == row.copy(inFlight = true)) {
            inventory.put(row)
            try { persistDelivery() } catch (e: Exception) { inventory.put(current); throw e }
            if (!closed) retryNotifyLater()
        }
    }
    private fun completeDelivery(target: DesktopBus, row: DeliveryRecord, epoch: Long, call: DesktopNotifyCall, handle: Long) {
        val current = inventory.get(row.key)
        val same = deliveryOwner == call.owner && current?.inFlight == true && current.version == row.version && current.actionKey == row.actionKey
        val visible = data.cache.view.value.let { it.sync != null && inventory.toCancel(it).none { old -> old.key == row.key } }
        if (same && !current!!.dismissed && identity.current(epoch) && visible) {
            inventory.posted(row.key, handle)
            try { persistDelivery() } catch (e: Exception) { inventory.put(current); throw e }
            target.acceptNotify(row.key, call, handle)
            resetNotifyRetry()
        } else {
            // Old epochs/retired rows never become actionable again, even if the replacement sees the same item.
            withdrawals += DesktopWithdrawal(call.owner, handle)
            if (same && epoch == (controller?.sessionEpoch?.value ?: 0L)) { inventory.remove(row.key); target.forget(row.key) }
            persistDelivery()
            if (!closed) drainWithdrawal() // shutdown only commits tombstones; native withdrawal must not consume its reply budget
        }
        busRevision.value++
        alarmWake.trySend(Unit)
    }
    private fun retireDelivery(row: DeliveryRecord) {
        // An unresolved handle-0 row after restart cannot be withdrawn: the protocol has no client-chosen id or
        // enumeration. Drop it on retirement without resending or blocking the next identity. A live pending reply
        // still owns its eventual handle and will persist a withdrawal, even when this row has already gone.
        if (row.handle > 0) deliveryOwner?.let { withdrawals += DesktopWithdrawal(it, row.handle) }
        bus?.forget(row.key)
        inventory.remove(row.key)
    }
    private fun drainWithdrawal(): Boolean {
        if (closed) return false
        val target = bus ?: return false
        val row = withdrawals.firstOrNull() ?: return true
        return try { target.withdraw(row.owner, row.handle); withdrawals.remove(row); persistDelivery(); true }
        catch (_: Exception) { setBusNotice(NOTIFICATION_NOTICE); false }
    }
    suspend fun prepareIdentity(owner: CacheOwner, epoch: Long = controller?.sessionEpoch?.value ?: 0L) {
        initializeInventory()
        identity.clear(epoch) {
            if (inventoryOwner?.owns(owner) != true) retireInventory()
            // The old/missing owner remains durable until BOTH inventories are retired.
            persistDelivery(owner)
            inventoryOwner = owner
        }
        withdrawRetired()
    }
    private fun retireInventory() {
        if (!alarmsAvailable) {
            ledger = alarmLoader(store)
            alarmsAvailable = true; alarmFailureNotice = null; publishNotice()
        }
        ledger.retire(); persist()
        inventory.records().forEach(::retireDelivery)
        persistDelivery() // retirement is unconditional, even without a daemon
    }
    suspend fun clearIdentity(epoch: Long = controller?.sessionEpoch?.value ?: 0L) {
        initializeInventory()
        identity.clear(epoch) { retireInventory(); persistDelivery(null); inventoryOwner = null }
        withdrawRetired()
    }
    private fun withdrawRetired() {
        // Withdraw-only tombstones survive disconnect/failure; no bus call delays identity retirement.
        alarmWake.trySend(Unit)
        jobs += scope.launch { mutex.withLock { repeat(MAX_NATIVE_UNITS) { if (!drainWithdrawal()) return@withLock } } }
    }
    private fun persistDelivery(owner: CacheOwner? = inventoryOwner) {
        deliveryWriter(deliveryFile, WireJson.encodeToString(DesktopDeliveryState(deliveryOwner, inventory.records(), withdrawals.toList(), owner)))
    }
    private fun persist() {
        if (!alarmsAvailable) { data.firedOccurrences.value = emptySet(); return }
        if (ledger.changes().isNotEmpty()) { store.save(ledger); alarmWake.trySend(Unit) }
        data.firedOccurrences.value = firedOccurrences(ledger.all())
    }
    private suspend fun handle(id: String, action: String) {
        val epoch = identity.epoch()
        data.cache.inSession {
            try {
                var selected: DeliveryRecord? = null
                var alarm: AlarmTicket? = null
                if (!identity.run(epoch) {
                    val record = inventory.get(id)?.takeIf { it.posted && !it.dismissed } ?: return@run
                    if (record.localAlarm) {
                        val ticket = ledger.ticket(id.removePrefix("alarm:").toInt())
                            ?.takeIf { it.active && it.generation == record.version } ?: return@run
                        alarm = ticket
                        if (action == "snooze") {
                            ledger.snooze(ticket.key, now() + 15.minutes); persist()
                            retireDelivery(record); persistDelivery(); drainWithdrawal()
                        } else if (action == "default") open(ticket.itemId, ticket.occurrenceId)
                    } else {
                        val row = data.cache.view.value.notifications.firstOrNull { it.id == id } ?: return@run
                        if (action == "default") open(row.itemId, row.occurrenceId)
                    }
                    selected = record
                }) return@inSession
                val record = selected ?: return@inSession
                if (!identity.current(epoch)) return@inSession
                val ticket = alarm
                if (ticket != null && action == "complete" && record.occurrenceVersion != null && record.actionKey != null) {
                    if (data.occurrences.complete(ticket.occurrenceId, record.occurrenceVersion, record.actionKey) is WriteResult.VersionConflict)
                        setNotice("Notification action could not complete. Open the item to review its current version.")
                } else if (!record.localAlarm && action == "read" && record.actionKey != null) data.notifications.read(id, record.actionKey)
            } catch (e: CancellationException) { throw e }
            catch (_: Exception) { setNotice("Notification action could not complete. Open Porchlight to retry.") }
        }
    }
    override fun close() {
        if (closed) return
        closed = true
        jobs.forEach { it.cancel() }; notifyRetryJob?.cancel()
        // Stop new native units first, then one total drain deadline (not one wait per reply). It outlasts the
        // longest already-running native unit's 2s bound, so ready handles can acquire the mutex and reach disk.
        // dbus-java connection teardown below has its own receiver-executor waits, outside this reply deadline.
        runBlocking {
            withTimeoutOrNull(2_500) {
                while (true) {
                    val drained = mutex.withLock {
                        pendingNotifies.toList().forEach { runCatching { finishNotify(it) } }
                        pendingNotifies.isEmpty()
                    }
                    if (drained) break
                    delay(10)
                }
            }
        }
        completions.cancel()
        try { bus?.close(); resumeMonitor?.close() } finally { pendingNotifies.clear() }
    }
}
private fun markupEscape(text: String) = text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
