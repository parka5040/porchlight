package app.porchlight.shared.trust

import app.porchlight.shared.live.ReadOnlyBusProperties
import org.freedesktop.dbus.DBusPath
import org.freedesktop.dbus.Tuple
import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.annotations.Position
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.connections.impl.DBusConnection
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.types.Variant
import org.junit.Assume.assumeTrue
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.test.*

/**
 * deskkey: the real dbus-java adapter against an in-process provider on a private `dbus-daemon` (never the user's
 * session bus or wallet). Covers what the in-memory fake cannot: Unlock's (ao, o) reply, Prompt.Prompt, the Completed
 * signal subscription, Dismiss on timeout, and dbus-java's ServiceUnknown mapping for the activation probe.
 * Skipped only where no dbus-daemon can run (some sandboxes); the controller's host run executes it.
 */
class SecretServiceDbusIntegrationTest {
    private class PrivateBus : AutoCloseable {
        val dir: Path = Files.createTempDirectory("pl-dbus") // short path: unix socket names are limited to 108 bytes
        private val process: Process
        val address: String
        init {
            val config = dir.resolve("bus.conf")
            Files.writeString(config, """
                <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
                 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
                <busconfig>
                  <type>session</type>
                  <listen>unix:path=${dir.resolve("bus")}</listen>
                  <auth>EXTERNAL</auth>
                  <policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy>
                </busconfig>
            """.trimIndent())
            process = ProcessBuilder("dbus-daemon", "--config-file=$config", "--nofork", "--print-address=1")
                .redirectError(ProcessBuilder.Redirect.DISCARD).start()
            // Bounded: a daemon that never prints its address must not hang the suite.
            address = try {
                val reader = process.inputStream.bufferedReader()
                java.util.concurrent.CompletableFuture.supplyAsync { reader.readLine() }.get(5, TimeUnit.SECONDS).orEmpty().trim()
            } catch (e: Exception) { close(); throw e }
        }
        fun connect(): DBusConnection = DBusConnectionBuilder.forAddress(address).withShared(false).build()
        override fun close() {
            process.destroy()
            if (!process.waitFor(5, TimeUnit.SECONDS)) process.destroyForcibly()
            dir.toFile().deleteRecursively()
        }
    }

    private fun privateBus(): PrivateBus {
        assumeTrue("dbus-daemon is not installed", listOf("/usr/bin/dbus-daemon", "/bin/dbus-daemon").any { Files.isExecutable(Path.of(it)) })
        val bus = runCatching { PrivateBus() }.getOrNull()
        if (bus == null || !bus.address.startsWith("unix:")) {
            bus?.close() // never leak the daemon or its folder on the skip path
            assumeTrue("a private dbus-daemon cannot start here", false)
        }
        return bus!!
    }

    @Test fun realAdapterUnlocksThroughTheProviderPromptAndHonoursDismissalAndTimeout() = privateBus().use { privateBus ->
        val client = DbusSecretServiceBus { privateBus.connect() }
        // No provider and nothing activatable: dbus-java must surface ServiceUnknown as "not activatable".
        assertFalse(secretServiceProviderExists(client) {})
        privateBus.connect().use { connection ->
            val provider = FakeProvider(connection)
            provider.export()
            assertTrue(secretServiceProviderExists(client) {})

            val wallet = DirectSecretService("app.porchlight.test", "dbus-integration", client, 5_000) {}
            provider.locked = true
            provider.answer = PromptAnswer.Complete
            wallet.write("secret-value")
            assertEquals(1, provider.unlocks); assertEquals(1, provider.prompts)
            assertFalse(provider.locked)
            assertEquals("secret-value", wallet.read())

            provider.locked = true
            provider.answer = PromptAnswer.Dismiss
            assertFailsWith<WalletUnlockDismissed> { wallet.read() }
            assertEquals(2, provider.prompts, "dismissal is final: no second prompt")
            assertTrue(provider.locked)

            provider.answer = PromptAnswer.Ignore
            val impatient = DirectSecretService("app.porchlight.test", "dbus-integration", client, 300) {}
            assertFailsWith<WalletUnlockDismissed> { impatient.read() }
            assertEquals(1, provider.dismissCalls, "a timed-out prompt is dismissed, not left on screen")

            provider.answer = PromptAnswer.Complete
            assertEquals("secret-value", wallet.read())
        }
    }
}

internal enum class PromptAnswer { Complete, Dismiss, Ignore }

// Provider-side views of the same wire interfaces: dbus-java exports multi-value replies only as generic tuples.
class Out2<A, B>(@field:Position(0) val first: A, @field:Position(1) val second: B) : Tuple()
@DBusInterfaceName("org.freedesktop.Secret.Service")
@JvmSuppressWildcards
interface ProviderService : DBusInterface {
    fun ReadAlias(name: String): DBusPath
    fun OpenSession(algorithm: String, input: Variant<String>): Out2<Variant<String>, DBusPath>
    fun SearchItems(attributes: Map<String, String>): Out2<List<DBusPath>, List<DBusPath>>
    fun Unlock(objects: List<DBusPath>): Out2<List<DBusPath>, DBusPath>
}
@DBusInterfaceName("org.freedesktop.Secret.Collection")
@JvmSuppressWildcards
interface ProviderCollection : DBusInterface {
    fun CreateItem(properties: Map<String, Variant<*>>, secret: SecretValue, replace: Boolean): Out2<DBusPath, DBusPath>
}

/** A minimal org.freedesktop.secrets provider: one default collection, a lock flag, and one-shot unlock prompts. */
internal class FakeProvider(private val connection: DBusConnection) : ProviderService {
    @Volatile var locked = false
    @Volatile var answer = PromptAnswer.Complete
    @Volatile var unlocks = 0
    @Volatile var prompts = 0
    @Volatile var dismissCalls = 0
    private val items = mutableMapOf<Map<String, String>, ByteArray>()
    private val collection = Collection()
    override fun getObjectPath() = "/org/freedesktop/secrets"
    fun export() {
        connection.exportObject(objectPath, this)
        connection.exportObject(COLLECTION, collection)
        connection.exportObject(SESSION, Session())
        connection.requestBusName(SECRET_SERVICE_NAME)
    }
    override fun ReadAlias(name: String) = DBusPath(if (name == "default") COLLECTION else "/")
    override fun OpenSession(algorithm: String, input: Variant<String>) = Out2(Variant(""), DBusPath(SESSION))
    override fun SearchItems(attributes: Map<String, String>): Out2<List<DBusPath>, List<DBusPath>> {
        val paths = synchronized(items) { if (attributes in items) listOf(DBusPath(itemPath(attributes))) else emptyList() }
        return if (locked) Out2(emptyList(), paths) else Out2(paths, emptyList())
    }
    override fun Unlock(objects: List<DBusPath>): Out2<List<DBusPath>, DBusPath> {
        unlocks++
        val path = "/org/freedesktop/secrets/prompt/p$unlocks"
        connection.exportObject(path, Prompt(path))
        return Out2(emptyList(), DBusPath(path))
    }
    private fun itemPath(attributes: Map<String, String>) = "$COLLECTION/i${attributes.hashCode().toUInt()}"

    inner class Prompt(private val path: String) : SecretPrompt {
        override fun getObjectPath() = path
        override fun Prompt(windowId: String) {
            prompts++
            val reply = answer
            if (reply == PromptAnswer.Ignore) return
            thread { // the user answers a moment after the dialog appears
                Thread.sleep(50)
                if (reply == PromptAnswer.Complete) locked = false
                connection.sendMessage(SecretPrompt.Completed(path, reply == PromptAnswer.Dismiss,
                    if (reply == PromptAnswer.Complete) Variant(listOf(DBusPath(COLLECTION)), "ao") else Variant("")))
            }
        }
        override fun Dismiss() { dismissCalls++ }
    }
    inner class Collection : ReadOnlyBusProperties("org.freedesktop.Secret.Collection"), ProviderCollection {
        override fun getObjectPath() = COLLECTION
        override fun properties(): Map<String, Variant<*>> = mapOf("Locked" to Variant(locked))
        override fun CreateItem(properties: Map<String, Variant<*>>, secret: SecretValue, replace: Boolean): Out2<DBusPath, DBusPath> {
            check(!locked)
            @Suppress("UNCHECKED_CAST")
            val attributes = properties.getValue("org.freedesktop.Secret.Item.Attributes").value as Map<String, String>
            synchronized(items) { items[attributes] = secret.value }
            connection.exportObject(itemPath(attributes), Item(attributes))
            return Out2(DBusPath(itemPath(attributes)), DBusPath("/"))
        }
    }
    inner class Item(private val attributes: Map<String, String>) : SecretItem {
        override fun getObjectPath() = itemPath(attributes)
        override fun GetSecret(session: DBusPath): SecretValue {
            check(!locked)
            return SecretValue(session, byteArrayOf(), synchronized(items) { items.getValue(attributes) }, "text/plain; charset=utf8")
        }
        override fun SetSecret(secret: SecretValue) { check(!locked); synchronized(items) { items[attributes] = secret.value } }
    }
    inner class Session : SecretSession {
        override fun getObjectPath() = SESSION
        override fun Close() {}
    }
    companion object {
        const val COLLECTION = "/org/freedesktop/secrets/collection/login"
        const val SESSION = "/org/freedesktop/secrets/session/s1"
    }
}
