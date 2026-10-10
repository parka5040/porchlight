package app.porchlight.shared.trust

import com.github.javakeyring.Keyring
import com.github.javakeyring.KeyringStorageType
import org.freedesktop.dbus.DBusPath
import org.freedesktop.dbus.Struct
import org.freedesktop.dbus.Tuple
import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.annotations.Position
import org.freedesktop.dbus.connections.impl.DBusConnection
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.errors.ServiceUnknown
import org.freedesktop.dbus.interfaces.DBus
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.interfaces.DBusSigHandler
import org.freedesktop.dbus.interfaces.Properties
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.UInt32
import org.freedesktop.dbus.types.Variant
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/** How long Porchlight waits for the user to answer the provider's unlock prompt before treating it as dismissed. */
internal const val SECRET_SERVICE_PROMPT_TIMEOUT_MILLIS = 120_000L

/** Prefer java-keyring's Secret Service backend; dbus-java 5 fallback. Neither ever writes to disk. */
class DesktopSecretWallet internal constructor(
    private val serviceName: String,
    private val accountName: String,
    private val direct: DirectSecretService,
    private val useKeyring: Boolean,
) : SecretWallet {
    constructor(serviceName: String = SERVICE, accountName: String = ACCOUNT) :
        this(serviceName, accountName, DirectSecretService(serviceName, accountName), useKeyring = true)
    override fun read(): String? = attempt { access({ getPassword(serviceName, accountName) }, { direct.read() }) }
    override fun write(value: String) { attempt { access({ setPassword(serviceName, accountName, value) }, { direct.write(value) }) } }
    override fun <T> attempt(block: () -> T): T = direct.attempt(block)
    private fun <T> access(block: Keyring.() -> T, fallback: () -> T): T {
        // Unlock first (the provider's own prompt if the collection is locked), so java-keyring never meets a locked one.
        direct.requireUnlocked()
        if (useKeyring) {
            try { return Keyring.create(KeyringStorageType.GNOME_KEYRING).use { it.block() } }
            catch (_: LinkageError) { /* incompatible dbus-java API */ }
            catch (_: Exception) { /* backend unavailable; use native D-Bus binding */ }
        }
        return fallback()
    }
    companion object { const val SERVICE = "app.porchlight.device"; const val ACCOUNT = "pkcs12-password" }
}

/**
 * Only the default collection. A locked one gets the standard Secret Service `Unlock` (owner decision 2026-10-07):
 * the provider (GNOME Keyring, KeePassXC, KWallet/ksecretd, ...) shows its own prompt; a dismissal or a prompt left
 * unanswered for [SECRET_SERVICE_PROMPT_TIMEOUT_MILLIS] is the recoverable [WalletUnlockDismissed]. A missing provider
 * or collection is [WalletUnavailable]. Never a file fallback: the backend was fixed at enrollment.
 */
class DirectSecretService internal constructor(
    serviceName: String,
    accountName: String,
    private val bus: SecretServiceBus,
    private val promptTimeoutMillis: Long,
    private val wait: () -> Unit,
) : SecretWallet {
    constructor(serviceName: String = DesktopSecretWallet.SERVICE, accountName: String = DesktopSecretWallet.ACCOUNT) :
        this(serviceName, accountName, DbusSecretServiceBus.Session, SECRET_SERVICE_PROMPT_TIMEOUT_MILLIS, secretServicePollWait)
    private val attrs = mapOf("service" to serviceName, "account" to accountName)
    private val budget = ThreadLocal<UnlockPromptBudget?>()
    /** One user-visible attempt (startup, enrollment, Retry): at most one unlock prompt across every call inside (M2). */
    override fun <T> attempt(block: () -> T): T {
        if (budget.get() != null) return block()
        budget.set(UnlockPromptBudget())
        try { return block() } finally { budget.remove() }
    }
    fun requireUnlocked() { connected { } }
    override fun read(): String? = connected { it.read(attrs) }
    override fun write(value: String) {
        connected { connection ->
            val prompt = connection.write(attrs, "Porchlight device key", value)
            if (prompt != NO_PROMPT) {
                // A shown CreateItem prompt is final either way: never re-run (and re-prompted) by the activation retry.
                val completed = try { connection.prompt(prompt, promptTimeoutMillis) } catch (e: Exception) { throw WalletUnlockDismissed(e) }
                if (!completed) throw WalletUnlockDismissed()
            }
        }
    }
    private fun <T> connected(operation: (SecretServiceConnection) -> T): T = attempt {
        val prompts = checkNotNull(budget.get())
        withSecretServiceActivation(bus, wait) {
            try {
                bus.connect().use { connection ->
                    connection.requireUnlockedDefault(promptTimeoutMillis, prompts)
                    operation(connection)
                }
            } catch (e: WalletUnavailable) { throw e }
            catch (e: Exception) { throw WalletUnavailable(e) }
        }
    }
}

/** Unlock prompts already shown in the current attempt (M2: at most one). */
internal class UnlockPromptBudget { var used = false }

internal const val NO_PROMPT = "/"

/** Secret Service `Unlock` reply: object paths unlocked at once, and the prompt to run ([NO_PROMPT] when none). */
internal data class SecretUnlock(val unlocked: List<String>, val prompt: String)

/**
 * The org.freedesktop.secrets calls Porchlight makes over one private session-bus connection (prompts must be run on
 * the connection that asked for them). Production: [DbusSecretServiceBus]; unit tests use an in-memory fake.
 */
internal interface SecretServiceConnection : AutoCloseable {
    /** Object path of the default collection, or null when the provider has no `default` alias. */
    fun defaultCollection(): String?
    fun isLocked(collection: String): Boolean
    fun unlock(collection: String): SecretUnlock
    /** `Prompt.Prompt("")`, then await `Completed`: true only when completed without dismissal; false on timeout. */
    fun prompt(path: String, timeoutMillis: Long): Boolean
    fun read(attributes: Map<String, String>): String?
    /** Creates or replaces the item in the default collection; returns the prompt to run ([NO_PROMPT] when none). */
    fun write(attributes: Map<String, String>, label: String, value: String): String
    override fun close()
}

internal interface SecretServiceBus : SecretServiceActivationBus {
    fun connect(): SecretServiceConnection
}

/**
 * Requests the provider's unlock prompt for a locked default collection, then confirms it is unlocked. Once a prompt
 * has been shown in this attempt, a second one is never requested, and a failing prompt call is final (M2).
 */
internal fun SecretServiceConnection.requireUnlockedDefault(promptTimeoutMillis: Long, budget: UnlockPromptBudget = UnlockPromptBudget()) {
    val collection = defaultCollection() ?: throw WalletUnavailable()
    if (!isLocked(collection)) return
    if (budget.used) throw WalletUnlockDismissed()
    val reply = unlock(collection)
    if (reply.prompt != NO_PROMPT) {
        budget.used = true
        val completed = try { prompt(reply.prompt, promptTimeoutMillis) } catch (e: Exception) { throw WalletUnlockDismissed(e) }
        if (!completed) throw WalletUnlockDismissed()
    }
    if (isLocked(collection)) throw WalletUnavailable()
}

// Public interfaces/structs are required by dbus-java's proxy and reflection marshaller.
@DBusInterfaceName("org.freedesktop.Secret.Service")
interface SecretService : DBusInterface {
    fun ReadAlias(name: String): DBusPath
    fun OpenSession(algorithm: String, input: Variant<String>): OpenSecretSession
    fun SearchItems(attributes: Map<String, String>): SecretSearch
    fun Unlock(objects: List<DBusPath>): UnlockedObjects
}
class OpenSecretSession(@field:Position(0) val output: Variant<*>, @field:Position(1) val session: DBusPath) : Tuple()
class SecretSearch(@field:Position(0) val unlocked: List<DBusPath>, @field:Position(1) val locked: List<DBusPath>) : Tuple()
class UnlockedObjects(@field:Position(0) val unlocked: List<DBusPath>, @field:Position(1) val prompt: DBusPath) : Tuple()
class SecretValue(@field:Position(0) val session: DBusPath, @field:Position(1) val parameters: ByteArray,
                  @field:Position(2) val value: ByteArray, @field:Position(3) val contentType: String) : Struct()
class CreatedSecret(@field:Position(0) val item: DBusPath, @field:Position(1) val prompt: DBusPath) : Tuple()
@DBusInterfaceName("org.freedesktop.Secret.Collection")
interface SecretCollection : DBusInterface {
    fun CreateItem(properties: Map<String, Variant<*>>, secret: SecretValue, replace: Boolean): CreatedSecret
}
@DBusInterfaceName("org.freedesktop.Secret.Item")
interface SecretItem : DBusInterface {
    fun GetSecret(session: DBusPath): SecretValue
    fun SetSecret(secret: SecretValue)
}
@DBusInterfaceName("org.freedesktop.Secret.Session")
interface SecretSession : DBusInterface { fun Close() }
@DBusInterfaceName("org.freedesktop.Secret.Prompt")
interface SecretPrompt : DBusInterface {
    fun Prompt(windowId: String)
    fun Dismiss()
    class Completed(path: String, val dismissed: Boolean, val result: Variant<*>) : DBusSignal(path, dismissed, result)
}

/**
 * A real bus (production: the session bus). Only a genuinely absent session bus is [SessionBusUnavailable] (M5); any
 * other failure to connect (refused, timed out, transport missing from a package) is an unusable [WalletUnavailable].
 */
internal class DbusSecretServiceBus(private val open: () -> DBusConnection) : SecretServiceBus {
    companion object {
        val Session = DbusSecretServiceBus {
            val address = sessionBusAddress(System::getenv) ?: throw SessionBusUnavailable(null)
            // A configured address keeps dbus-java's own resolution; otherwise the standard $XDG_RUNTIME_DIR/bus socket.
            if (System.getenv("DBUS_SESSION_BUS_ADDRESS").isNullOrBlank()) DBusConnectionBuilder.forAddress(address).withShared(false).build()
            else DBusConnectionBuilder.forSessionBus().withShared(false).build()
        }
    }
    private fun session(): DBusConnection = try { open() }
        catch (e: SessionBusUnavailable) { throw e }
        catch (e: Exception) { throw WalletUnavailable(e) }
    private fun DBusConnection.daemon() = getRemoteObject("org.freedesktop.DBus", "/org/freedesktop/DBus", DBus::class.java)
    override fun hasOwner(name: String): Boolean = session().use { it.daemon().NameHasOwner(name) }
    override fun start(name: String): Boolean = session().use { conn ->
        try { conn.daemon().StartServiceByName(name, UInt32(0)); true }
        catch (_: ServiceUnknown) { false }
    }
    override fun connect(): SecretServiceConnection = session().let { conn ->
        try { DbusSecretServiceConnection(conn) } catch (e: Exception) { conn.close(); throw e }
    }
}

private class DbusSecretServiceConnection(private val conn: DBusConnection) : SecretServiceConnection {
    private val bus = SECRET_SERVICE_NAME
    private val service = conn.getRemoteObject(bus, "/org/freedesktop/secrets", SecretService::class.java)
    override fun defaultCollection(): String? = service.ReadAlias("default").path.takeIf { it != NO_PROMPT }
    override fun isLocked(collection: String): Boolean =
        conn.getRemoteObject(bus, collection, Properties::class.java).Get<Boolean>("org.freedesktop.Secret.Collection", "Locked")
    override fun unlock(collection: String): SecretUnlock =
        service.Unlock(listOf(DBusPath(collection))).let { reply -> SecretUnlock(reply.unlocked.map { it.path }, reply.prompt.path) }
    override fun prompt(path: String, timeoutMillis: Long): Boolean {
        val prompt = conn.getRemoteObject(bus, path, SecretPrompt::class.java)
        val completed = CompletableFuture<Boolean>()
        // Subscribe before Prompt() so a fast Completed cannot be missed.
        val subscription = conn.addSigHandler(SecretPrompt.Completed::class.java, prompt,
            DBusSigHandler { signal -> if (signal.path == path) completed.complete(!signal.dismissed) })
        try {
            prompt.Prompt("")
            return try { completed.get(timeoutMillis, TimeUnit.MILLISECONDS) }
            catch (_: TimeoutException) { runCatching { prompt.Dismiss() }; false }
        } finally { runCatching { subscription.close() } }
    }
    private fun <T> session(block: (DBusPath) -> T): T {
        val session = service.OpenSession("plain", Variant("")).session
        try { return block(session) }
        finally { runCatching { conn.getRemoteObject(bus, session.path, SecretSession::class.java).Close() } }
    }
    private fun item(attributes: Map<String, String>): SecretItem? {
        val search = service.SearchItems(attributes)
        if (search.locked.isNotEmpty() || search.unlocked.size > 1) throw WalletUnavailable()
        return search.unlocked.firstOrNull()?.let { conn.getRemoteObject(bus, it.path, SecretItem::class.java) }
    }
    override fun read(attributes: Map<String, String>): String? = session { id ->
        item(attributes)?.GetSecret(id)?.value?.toString(Charsets.UTF_8)
    }
    override fun write(attributes: Map<String, String>, label: String, value: String): String = session { id ->
        val secret = SecretValue(id, byteArrayOf(), value.toByteArray(), "text/plain; charset=utf8")
        val existing = item(attributes)
        if (existing != null) { existing.SetSecret(secret); NO_PROMPT }
        else {
            val collection = conn.getRemoteObject(bus, service.ReadAlias("default").path, SecretCollection::class.java)
            collection.CreateItem(mapOf("org.freedesktop.Secret.Item.Label" to Variant(label),
                "org.freedesktop.Secret.Item.Attributes" to Variant(attributes, "a{ss}")), secret, true).prompt.path
        }
    }
    override fun close() = conn.close()
}
