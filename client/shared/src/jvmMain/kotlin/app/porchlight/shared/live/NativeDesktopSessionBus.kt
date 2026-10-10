package app.porchlight.shared.live

import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.*
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.*

internal class NativeDesktopSessionBus : DesktopSessionBus {
    @Volatile private var listener: DesktopBusEvents? = null
    @Volatile private var lost = false
    private val connection = DBusConnectionBuilder.forSessionBus().withShared(false)
        .withDisconnectCallback(object : org.freedesktop.dbus.connections.IDisconnectCallback {
            override fun disconnectOnError(error: java.io.IOException) { lost = true; listener?.disconnected() }
            override fun exceptionOnTerminate(error: java.io.IOException) { lost = true; listener?.disconnected() }
        }).build()
    private val daemon = connection.getRemoteObject("org.freedesktop.DBus", "/org/freedesktop/DBus", DBus::class.java)
    override fun owner(): String? = if (daemon.NameHasOwner("org.freedesktop.Notifications")) daemon.GetNameOwner("org.freedesktop.Notifications") else null
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
        connection.getRemoteObject("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", TrayWatcher::class.java).RegisterStatusNotifierItem(connection.uniqueName)
    }
    private fun notifications(owner: String) = connection.getRemoteObject(owner, "/org/freedesktop/Notifications", DesktopNotifications::class.java)
    override fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>) =
        notifications(owner).Notify("Porchlight", UInt32(replaces), "porchlight", title, body, actions,
            mapOf("urgency" to Variant(1.toByte())), -1).toLong()
    override fun dismiss(owner: String, id: Long) = notifications(owner).CloseNotification(UInt32(id))
    override fun close() = connection.close()
}
