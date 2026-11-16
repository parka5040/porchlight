package app.porchlight.shared.live

import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.DBus
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.interfaces.DBusSigHandler
import org.freedesktop.dbus.messages.DBusSignal
import kotlinx.coroutines.*

@DBusInterfaceName("org.freedesktop.login1.Manager")
interface DesktopLoginManager : DBusInterface {
    class PrepareForSleep(path: String, val sleeping: Boolean) : DBusSignal(path, sleeping)
}

/** logind lives on the system bus, independent of the tray's session bus. */
interface DesktopSleepBus : AutoCloseable {
    fun watch(receive: (Boolean) -> Unit)
    fun watchLoss(lost: () -> Unit) {}
}
internal class NativeDesktopSleepBus : DesktopSleepBus {
    @Volatile private var lost: (() -> Unit)? = null
    @Volatile private var disconnected = false
    private val connection = DBusConnectionBuilder.forSystemBus().withShared(false)
        .withDisconnectCallback(object : org.freedesktop.dbus.connections.IDisconnectCallback {
            override fun disconnectOnError(error: java.io.IOException) { disconnected = true; lost?.invoke() }
            override fun exceptionOnTerminate(error: java.io.IOException) { disconnected = true; lost?.invoke() }
        }).build()
    private val daemon = try {
        connection.getRemoteObject("org.freedesktop.DBus", "/org/freedesktop/DBus", DBus::class.java)
    } catch (e: Exception) { connection.close(); throw e }
    override fun watchLoss(lost: () -> Unit) { this.lost = lost; if (disconnected) lost() }
    override fun watch(receive: (Boolean) -> Unit) {
        connection.addSigHandler(DesktopLoginManager.PrepareForSleep::class.java, DBusSigHandler { event ->
            val owner = runCatching { daemon.GetNameOwner("org.freedesktop.login1") }.getOrNull()
            if (event.path == "/org/freedesktop/login1" && owner != null && event.source == owner) receive(event.sleeping)
        })
    }
    override fun close() = connection.close()
}

/** Missing logind keeps the normal watchdog fallback; it must not disable Inbox or alarms.
 * Resume reconnects the long-lived LiveSync from its acknowledged cursor. Its own negotiated replay applies
 * the same targeted prefix as bounded catch-up; a second short-lived socket would only duplicate that work.
 */
internal class DesktopResumeMonitor(private val networkChanged: () -> Unit,
    scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.IO),
    private val factory: () -> DesktopSleepBus = { NativeDesktopSleepBus() }) : AutoCloseable {
    private var bus: DesktopSleepBus? = null
    private var lost = CompletableDeferred<Unit>()
    private fun connect(): Boolean = runCatching {
        val candidate = factory()
        try {
            val signal = lost
            candidate.watchLoss { signal.complete(Unit) }
            candidate.watch { sleeping -> if (!sleeping) networkChanged() }
            bus = candidate
        } catch (e: Exception) { candidate.close(); throw e }
        true
    }.getOrDefault(false)
    private val connected = connect()
    private val job = scope.launch {
        var backoff = 1_000L
        var available = connected
        try {
            while (isActive) {
                if (available) {
                    lost.await()
                    runCatching { bus?.close() }; bus = null
                    lost = CompletableDeferred()
                    backoff = 1_000L
                }
                delay(backoff)
                available = connect()
                if (!available) backoff = (backoff * 2).coerceAtMost(60_000L)
            }
        } finally { runCatching { bus?.close() }; bus = null }
    }
    override fun close() { job.cancel(); runCatching { bus?.close() }; bus = null }
}
