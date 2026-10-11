package app.porchlight.shared.live

import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.*

@DBusInterfaceName("org.freedesktop.Notifications")
interface DesktopNotifications : DBusInterface {
    fun Notify(appName: String, replacesId: UInt32, appIcon: String, summary: String, body: String,
        // Boxed on the JVM for dbus-java's runtime-argument callback lookup; still D-Bus signature 'i'.
        actions: List<String>, hints: Map<String, Variant<*>>, expireTimeout: Int?): UInt32
    fun CloseNotification(id: UInt32)
    class NotificationClosed(path: String, val id: UInt32, val reason: UInt32) : DBusSignal(path, id, reason)
    class ActionInvoked(path: String, val id: UInt32, val key: String) : DBusSignal(path, id, key)
}
@DBusInterfaceName("org.kde.StatusNotifierWatcher")
interface TrayWatcher : DBusInterface { fun RegisterStatusNotifierItem(service: String) }

interface DesktopBusEvents {
    fun disconnected() {}
    fun ownerChanged(owner: String?)
    fun watcherChanged()
    fun action(sender: String, id: Long, key: String)
    fun closed(sender: String, id: Long, reason: Long)
}
/** Injectable session bus; production and deterministic in-process bus use identical exported objects. */
interface DesktopSessionBus : AutoCloseable {
    fun owner(): String?
    fun export(path: String, value: DBusInterface)
    fun emit(signal: DBusSignal)
    fun watch(events: DesktopBusEvents)
    fun registerTray()
    fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>): Long
    fun dismiss(owner: String, id: Long)
}
/** No notification was created: either submission never happened or the daemon explicitly rejected it. */
internal class DesktopNotifyRejected(cause: Throwable) : java.io.IOException(cause.message, cause)

internal interface AsyncDesktopSessionBus : DesktopSessionBus {
    // Throw only before submission. Once submitted, complete with a handle, DesktopNotifyRejected for a daemon
    // rejection, or an unclassified exception for an ambiguous transport failure. A pending future stays owned.
    fun notifyAsync(owner: String, replaces: Long, title: String, body: String, actions: List<String>): java.util.concurrent.CompletableFuture<Long>
}
fun desktopBusOrNull(create: () -> DesktopBus): DesktopBus? = try { create() } catch (_: Exception) { null }

internal data class DesktopNotifyCall(val owner: String, val reply: java.util.concurrent.CompletableFuture<Long>, val actions: Set<String>) {
    fun await(): Long = reply.get(2_000, java.util.concurrent.TimeUnit.MILLISECONDS)
}

/** No callbacks run under the handle lock. Owners fence stale signals and reused notification ids. */
class DesktopBus(private val open: () -> Unit, quit: () -> Unit,
    private val action: (String, String) -> Unit, private val dismissed: (String) -> Unit = {},
    private val session: DesktopSessionBus = NativeDesktopSessionBus(),
    trayActions: DesktopTrayActions = DesktopTrayActions(), private val serverChanged: (String?) -> Unit = {},
    private val degraded: () -> Unit = {}, private val connectionLost: () -> Unit = {}) : AutoCloseable {
    private val lock = Any()
    private val ids = mutableMapOf<String, Long>()
    private val reverse = mutableMapOf<Long, String>()
    private val allowed = mutableMapOf<String, Set<String>>()
    @Volatile var notificationServerOwner: String? = null; private set
    @Volatile var trayAvailable: Boolean = false; private set
    private fun registerTray() {
        try { session.registerTray(); trayAvailable = true }
        catch (_: Exception) { trayAvailable = false; degraded() }
    }
    private val tray = DesktopTray(open, trayActions) { session.emit(it) }
    private val menu = DesktopMenu(open, quit, trayActions) { session.emit(it) }
    private var closed = false
    init {
        try {
            session.export("/StatusNotifierItem", tray); session.export("/Menu", menu)
            // Subscribe before querying the owner; refreshOwner rechecks to fence query/event races.
            session.watch(object : DesktopBusEvents {
                override fun disconnected() { connectionLost() }
                override fun ownerChanged(owner: String?) { refreshOwner() }
                override fun watcherChanged() { registerTray(); serverChanged(notificationServerOwner) }
                override fun action(sender: String, id: Long, key: String) {
                    val row = synchronized(lock) {
                        if (closed || sender != notificationServerOwner) null
                        else reverse[id]?.takeIf { key in (allowed[it] ?: emptySet()) }
                    }
                    if (row != null) action(row, key) // replay deliberately retains the original durable operation key
                }
                override fun closed(sender: String, id: Long, reason: Long) {
                    val row = synchronized(lock) {
                        if (closed || sender != notificationServerOwner) null
                        else reverse.remove(id)?.also { ids.remove(it); allowed.remove(it) }
                    }
                    // Expiry/server closure is not a user dismissal; permit recovery/redelivery.
                    if (row != null && reason == 2L) dismissed(row)
                }
            })
            refreshOwner(false)
            registerTray()
        } catch (e: Exception) { menu.close(); session.close(); throw e }
    }
    fun refreshOwner(report: Boolean = true) {
        val next = session.owner()
        if (report && !trayAvailable) registerTray()
        val changed = synchronized(lock) {
            if (closed || next == notificationServerOwner) false else {
                notificationServerOwner = next; ids.clear(); reverse.clear(); allowed.clear(); true
            }
        }
        if (changed && report) serverChanged(next)
    }
    fun updateStatus(unreachable: Boolean, unread: Int) = tray.update(unreachable, unread)
    fun restoreHandles(records: List<DeliveryRecord>) = synchronized(lock) {
        records.filter { it.handle > 0 && !it.dismissed }.forEach { row ->
            ids[row.key] = row.handle; reverse[row.handle] = row.key
            allowed[row.key] = if (!row.posted) emptySet() else if (row.localAlarm) setOf("default", "snooze", "complete") else setOf("default", "read")
        }
    }
    internal fun beginNotify(key: String, title: String, body: String, actions: List<String>): DesktopNotifyCall {
        val (owner, replaces) = try {
            require(actions.size % 2 == 0)
            synchronized(lock) {
                check(!closed)
                (notificationServerOwner ?: throw IllegalStateException("Notifications unavailable")) to (ids[key] ?: 0)
            }
        } catch (e: Exception) { throw DesktopNotifyRejected(e) }
        val reply = if (session is AsyncDesktopSessionBus) {
            try { session.notifyAsync(owner, replaces, title, body, actions) }
            catch (e: Exception) { throw DesktopNotifyRejected(e) }
        } else try { java.util.concurrent.CompletableFuture.completedFuture(session.notify(owner, replaces, title, body, actions)) }
            catch (e: Exception) { java.util.concurrent.CompletableFuture.failedFuture(e) }
        return DesktopNotifyCall(owner, reply, actions.chunked(2).map { it[0] }.toSet())
    }
    internal fun acceptNotify(key: String, call: DesktopNotifyCall, id: Long) = synchronized(lock) {
        if (!closed && notificationServerOwner == call.owner) {
            ids[key]?.let { reverse.remove(it) }; ids[key] = id; reverse[id] = key; allowed[key] = call.actions
        }
    }
    fun notify(key: String, title: String, body: String, actions: List<String> = listOf("default", "Open", "read", "Got it")): Long {
        val call = beginNotify(key, title, body, actions)
        try { return call.await().also { acceptNotify(key, call, it) } }
        catch (e: java.util.concurrent.TimeoutException) {
            // Callers without a durable inventory can only withdraw an ambiguous late result.
            call.reply.thenAcceptAsync { session.dismiss(call.owner, it) }
            throw e
        }
    }
    fun dismiss(key: String) = synchronized(lock) {
        ids.remove(key)?.let { id -> reverse.remove(id); notificationServerOwner?.let { session.dismiss(it, id) } }
        allowed.remove(key); Unit
    }
    /** Detach actions immediately; persisted owner-bound withdrawals are never notification candidates. */
    fun forget(key: String) = synchronized(lock) {
        ids.remove(key)?.let { reverse.remove(it) }; allowed.remove(key); Unit
    }
    fun withdraw(owner: String, id: Long) = synchronized(lock) {
        val current = notificationServerOwner ?: throw IllegalStateException("Notifications unavailable")
        if (current == owner) session.dismiss(owner, id)
    }
    override fun close() {
        synchronized(lock) { closed = true; ids.clear(); reverse.clear(); allowed.clear() }
        menu.close(); session.close()
    }
}
