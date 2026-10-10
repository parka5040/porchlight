package app.porchlight.shared.theme

import app.porchlight.shared.reportDiagnostic
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.*
import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.interfaces.DBusSigHandler
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.Variant

fun portalPrefersDark(value: Any?): Boolean = when (value) {
    is Variant<*> -> portalPrefersDark(value.value)
    is Number -> value.toInt() != 2 // 0 = no preference, 1 = dark, 2 = light
    else -> true
}
fun interface PortalAppearanceSource { fun readings(): Flow<Any?> }

/** Starts dark immediately. All portal IO happens off the UI thread; cancellation closes the subscription. */
class DesktopPortalTheme(scope: CoroutineScope, source: PortalAppearanceSource = DbusPortalAppearanceSource(),
    dispatcher: CoroutineDispatcher = Dispatchers.IO) {
    private val state = MutableStateFlow(true)
    val dark: StateFlow<Boolean> = state.asStateFlow()
    init {
        scope.launch(dispatcher) {
            try { source.readings().collect { state.value = portalPrefersDark(it) } }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                reportDiagnostic("Desktop appearance portal (dark fallback)", e)
                state.value = true
            }
        }
    }
}

@DBusInterfaceName("org.freedesktop.portal.Settings")
interface PortalSettings : DBusInterface {
    fun Read(namespace: String, key: String): Variant<*>
    class SettingChanged(path: String, val namespace: String, val key: String, val value: Variant<*>) :
        DBusSignal(path, namespace, key, value)
}

/** Thin transport seam: tests reproduce dbus-java's unique-source matching, not just a fake theme flow. */
interface PortalTransport : AutoCloseable {
    fun subscribe(sender: String?, handler: (PortalSettings.SettingChanged) -> Unit): AutoCloseable
    fun read(): Any?
}

private class SessionPortalTransport : PortalTransport {
    private val connection = DBusConnectionBuilder.forSessionBus().withShared(false).build()
    private val settings = try {
        connection.getRemoteObject("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", PortalSettings::class.java)
    } catch (e: Exception) { connection.close(); throw e }

    override fun subscribe(sender: String?, handler: (PortalSettings.SettingChanged) -> Unit): AutoCloseable =
        if (sender == null) {
            // Interface/member/path match only. The signal source is :1.N, never the well-known service name.
            connection.addSigHandler(PortalSettings.SettingChanged::class.java, settings, DBusSigHandler { handler(it) })
        } else {
            connection.addSigHandler(PortalSettings.SettingChanged::class.java, sender, settings, DBusSigHandler { handler(it) })
        }
    override fun read(): Any? = settings.Read("org.freedesktop.appearance", "color-scheme")
    override fun close() = connection.close()
}

class DbusPortalAppearanceSource(private val connect: () -> PortalTransport = { SessionPortalTransport() }) : PortalAppearanceSource {
    override fun readings(): Flow<Any?> = callbackFlow {
        val transport = connect()
        val subscription = try {
            transport.subscribe(null) { signal ->
                if (signal.namespace == "org.freedesktop.appearance" && signal.key == "color-scheme") trySend(signal.value)
            }
        } catch (e: Exception) { transport.close(); throw e }
        try {
            try { trySend(transport.read()) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                reportDiagnostic("Reading desktop appearance (listening for later changes)", e)
                trySend(null) // Missing Read key is not a reason to unsubscribe.
            }
            awaitClose { }
        } finally {
            try { subscription.close() } finally { transport.close() }
        }
    }
}
