package app.porchlight.shared.live

import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.*
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.*

internal class NativeDesktopSessionBus(address: String? = null) : AsyncDesktopSessionBus {
    @Volatile private var listener: DesktopBusEvents? = null
    @Volatile private var lost = false
    private val submissionLock = Any()
    private val pending = java.util.concurrent.ConcurrentHashMap.newKeySet<java.util.concurrent.CompletableFuture<Long>>()
    private fun abandonReplies() {
        val abandoned = synchronized(submissionLock) { lost = true; pending.toList() }
        // dbus-java marks MethodCalls on disconnect but never invokes their asynchronous callbacks.
        abandoned.forEach { it.completeExceptionally(java.io.IOException("Notification connection lost after submission")) }
    }
    // Bound reply waits on THIS bus only. dbus-java's MethodCall reply timeout is process-wide and also governs the
    // Secret Service key store, portal and logind proxies (D-Bus activation of a keyring can exceed a second).
    private val calls = java.util.concurrent.Executors.newSingleThreadExecutor { task ->
        Thread(task, "porchlight-notification-bus").apply { isDaemon = true }
    }
    private fun <T> bounded(block: () -> T): T {
        val reply = calls.submit(java.util.concurrent.Callable(block))
        try { return reply.get(CALL_TIMEOUT_MS, java.util.concurrent.TimeUnit.MILLISECONDS) }
        catch (e: java.util.concurrent.TimeoutException) {
            reply.cancel(true); throw java.io.IOException("Notification service did not reply in time", e)
        }
        catch (e: java.util.concurrent.ExecutionException) { throw (e.cause as? Exception) ?: e }
    }
    private val connection = (address?.let(DBusConnectionBuilder::forAddress) ?: DBusConnectionBuilder.forSessionBus()).withShared(false)
        .transportConfig().withTimeout(1_000).back()
        .withDisconnectCallback(object : org.freedesktop.dbus.connections.IDisconnectCallback {
            override fun disconnectOnError(error: java.io.IOException) { abandonReplies(); listener?.disconnected() }
            override fun exceptionOnTerminate(error: java.io.IOException) { abandonReplies(); listener?.disconnected() }
        }).build()
    private val daemon = connection.getRemoteObject("org.freedesktop.DBus", "/org/freedesktop/DBus", DBus::class.java)
    override fun owner(): String? = bounded { if (daemon.NameHasOwner("org.freedesktop.Notifications")) daemon.GetNameOwner("org.freedesktop.Notifications") else null }
    override fun export(path: String, value: DBusInterface) = connection.exportObject(path, value)
    override fun emit(signal: DBusSignal) { connection.sendMessage(signal) }
    override fun watch(events: DesktopBusEvents) {
        listener = events
        if (lost) { events.disconnected(); return }
        connection.addSigHandler(DBus.NameOwnerChanged::class.java, DBusSigHandler { event ->
            when (event.name) {
                "org.freedesktop.Notifications" -> events.ownerChanged(event.newOwner.takeIf { it.isNotEmpty() })
                "org.kde.StatusNotifierWatcher" -> events.watcherChanged()
            }
        })
        connection.addSigHandler(DesktopNotifications.ActionInvoked::class.java, DBusSigHandler { event ->
            if (event.path == "/org/freedesktop/Notifications") events.action(event.source, event.id.toLong(), event.key)
        })
        connection.addSigHandler(DesktopNotifications.NotificationClosed::class.java, DBusSigHandler { event ->
            if (event.path == "/org/freedesktop/Notifications") events.closed(event.source, event.id.toLong(), event.reason.toLong())
        })
    }
    override fun registerTray() {
        bounded { connection.getRemoteObject("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", TrayWatcher::class.java).RegisterStatusNotifierItem(connection.uniqueName) }
    }
    private fun notifications(owner: String) = connection.getRemoteObject(owner, "/org/freedesktop/Notifications", DesktopNotifications::class.java)
    override fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>): Long =
        error("Native notifications require an owned asynchronous reply")
    override fun notifyAsync(owner: String, replaces: Long, title: String, body: String, actions: List<String>): java.util.concurrent.CompletableFuture<Long> = synchronized(submissionLock) {
        if (lost) return@synchronized java.util.concurrent.CompletableFuture.failedFuture(DesktopNotifyRejected(java.io.IOException("Notification connection closed")))
        val reply = java.util.concurrent.CompletableFuture<Long>()
        pending += reply
        reply.whenComplete { _, _ -> pending.remove(reply) }
        try {
            // No synchronous MethodCall timeout: the callback owns a reply even after our per-bus wait expires.
            connection.callWithCallback(notifications(owner), "Notify", object : CallbackHandler<UInt32> {
                override fun handle(value: UInt32) { reply.complete(value.toLong()) }
                override fun handleError(error: org.freedesktop.dbus.exceptions.DBusExecutionException) {
                    val info = org.freedesktop.dbus.connections.base.AbstractConnectionBase.getCallInfo()
                    // 5.2.2 maps known error names to exception classes, but erases unknown names (e.g. TimedOut).
                    // Error callback call-info retains the wire error name in method; never classify by message text.
                    val name = info?.method ?: error.type.replace("org.freedesktop.dbus.errors.", "org.freedesktop.DBus.Error.")
                    val ambiguous = name in setOf("NoReply", "Timeout", "TimedOut", "Disconnected").map { "org.freedesktop.DBus.Error.$it" }
                    val undelivered = name in setOf("org.freedesktop.DBus.Error.ServiceUnknown", "org.freedesktop.DBus.Error.NameHasNoOwner")
                    val definite = !ambiguous && (undelivered || info?.source == owner)
                    reply.completeExceptionally(if (definite) DesktopNotifyRejected(error) else error)
                }
            }, "Porchlight", UInt32(replaces), "porchlight", title, body, actions, mapOf("urgency" to Variant(1.toByte())), -1)
        } catch (e: Exception) {
            // Lookup/marshalling/submission threw before sendMessage handed the call to its sender executor.
            reply.completeExceptionally(DesktopNotifyRejected(e))
        }
        reply
    }
    override fun dismiss(owner: String, id: Long) = bounded { notifications(owner).CloseNotification(UInt32(id)) }
    override fun close() { abandonReplies(); calls.shutdownNow(); connection.close() }
    private companion object { const val CALL_TIMEOUT_MS = 2_000L }
}
