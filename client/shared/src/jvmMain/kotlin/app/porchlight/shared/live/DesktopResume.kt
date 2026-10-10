package app.porchlight.shared.live

import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.DBus
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.interfaces.DBusSigHandler
import org.freedesktop.dbus.messages.DBusSignal

@DBusInterfaceName("org.freedesktop.login1.Manager")
interface DesktopLoginManager : DBusInterface {
    class PrepareForSleep(path: String, val sleeping: Boolean) : DBusSignal(path, sleeping)
}

/** logind lives on the system bus, independent of the tray's session bus. */
interface DesktopSleepBus : AutoCloseable {
    fun watch(receive: (Boolean) -> Unit)
}
internal class NativeDesktopSleepBus : DesktopSleepBus {
    private val connection = DBusConnectionBuilder.forSystemBus().withShared(false).build()
    private val daemon = connection.getRemoteObject("org.freedesktop.DBus", "/org/freedesktop/DBus", DBus::class.java)
    override fun watch(receive: (Boolean) -> Unit) {
        connection.addSigHandler(DesktopLoginManager.PrepareForSleep::class.java, DBusSigHandler { event ->
            val owner = runCatching { daemon.GetNameOwner("org.freedesktop.login1") }.getOrNull()
            if (event.path == "/org/freedesktop/login1" && owner != null && event.source == owner) receive(event.sleeping)
        })
    }
    override fun close() = connection.close()
}

/** Missing logind keeps the normal watchdog fallback; it must not disable Inbox or alarms. */
internal class DesktopResumeMonitor(networkChanged: () -> Unit,
    factory: () -> DesktopSleepBus = { NativeDesktopSleepBus() }) : AutoCloseable {
    private val bus = runCatching {
        val candidate = factory()
        try { candidate.watch { sleeping -> if (!sleeping) networkChanged() }; candidate }
        catch (e: Exception) { candidate.close(); throw e }
    }.getOrNull()
    override fun close() { runCatching { bus?.close() } }
}
