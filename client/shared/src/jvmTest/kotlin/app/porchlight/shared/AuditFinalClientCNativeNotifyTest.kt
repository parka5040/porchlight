package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.emptySnapshot
import org.freedesktop.dbus.annotations.DBusInterfaceName
import org.freedesktop.dbus.bin.EmbeddedDBusDaemon
import org.freedesktop.dbus.connections.impl.DBusConnectionBuilder
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.types.*
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.util.concurrent.*
import kotlin.test.*

// Independent service declaration checks the wire signature, not just our proxy's Java parameter types.
@DBusInterfaceName("org.freedesktop.Notifications")
interface AuditFinalClientCNotifications : DBusInterface {
    fun Notify(appName: String, replacesId: UInt32, appIcon: String, summary: String, body: String,
        actions: List<String>, hints: Map<String, Variant<*>>, expireTimeout: Int): UInt32
    fun CloseNotification(id: UInt32)
}

class AuditFinalClientCNativeNotifyTest {
    private class Service : AuditFinalClientCNotifications {
        val attempts = java.util.concurrent.atomic.AtomicInteger()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val dismissed = CompletableFuture<Long>()
        @Volatile var slow = false
        @Volatile var reject = false
        @Volatile var values: List<Any>? = null
        override fun Notify(appName: String, replacesId: UInt32, appIcon: String, summary: String, body: String,
            actions: List<String>, hints: Map<String, Variant<*>>, expireTimeout: Int): UInt32 {
            attempts.incrementAndGet()
            values = listOf(appName, replacesId.toLong(), appIcon, summary, body, actions, hints.getValue("urgency").value, expireTimeout)
            entered.countDown()
            if (slow) check(release.await(6, TimeUnit.SECONDS))
            if (reject) throw org.freedesktop.dbus.errors.AccessDenied("Service rejected delivery")
            return UInt32(42)
        }
        override fun CloseNotification(id: UInt32) { dismissed.complete(id.toLong()) }
        override fun getObjectPath() = "/org/freedesktop/Notifications"
    }
    private fun withBus(test: (NativeDesktopSessionBus, Service, EmbeddedDBusDaemon, String) -> Unit) {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "bus-",
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
        // A relative socket path avoids sockaddr_un's path limit in long tree-copy directories.
        val socket = Path.of("").toAbsolutePath().relativize(dir.resolve("socket").toAbsolutePath())
        val address = "unix:path=$socket"
        val daemon = EmbeddedDBusDaemon(org.freedesktop.dbus.connections.BusAddress.of(address).listenerAddress)
        val ready = CompletableFuture<Unit>()
        daemon.setBindCallback { ready.complete(Unit) }
        val runner = Thread({
            try { daemon.startInForeground() } catch (e: Throwable) { ready.completeExceptionally(e) }
        }, "test-notification-daemon").apply { isDaemon = true; start() }
        try {
            ready.get(3, TimeUnit.SECONDS)
            DBusConnectionBuilder.forAddress(address).withShared(false).build().use { serviceBus ->
                val service = Service()
                serviceBus.exportObject(service.objectPath, service)
                serviceBus.requestBusName("org.freedesktop.Notifications")
                NativeDesktopSessionBus(address).use { native ->
                    try { test(native, service, daemon, address) } finally { service.release.countDown() }
                }
            }
        } finally { daemon.close(); runner.join(3_000); dir.toFile().deleteRecursively() }
    }
    @Test fun realAdapterPostsExactWireArgumentsAndReturnsHandle() = withBus { native, service, _, _ ->
        val actions = listOf("default", "Open", "read", "Got it")
        val reply = native.notifyAsync(checkNotNull(native.owner()), 7, "Household alert", "Body &lt;safe&gt;", actions)
        assertEquals(42L, reply.get(3, TimeUnit.SECONDS), "Real Notify must resolve its boxed runtime arguments")
        assertEquals(listOf("Porchlight", 7L, "porchlight", "Household alert", "Body &lt;safe&gt;", actions, 1.toByte(), -1), service.values)
    }
    @Test fun realAdapterOwnsReplyBeyondLocalBoundWithoutBlockingOtherCalls() = withBus { native, service, _, _ ->
        service.slow = true
        val owner = checkNotNull(native.owner())
        val reply = native.notifyAsync(owner, 0, "Reminder", "Household reminder", listOf("default", "Open"))
        assertTrue(service.entered.await(3, TimeUnit.SECONDS), "Native Notify must reach the exported daemon")
        val call = DesktopNotifyCall(owner, reply, setOf("default"))
        assertFailsWith<TimeoutException> { call.await() }
        assertFalse(reply.isDone, "Local bound must not cancel the owned remote reply")
        assertEquals(owner, native.owner())
        native.dismiss(owner, 9)
        assertEquals(9L, service.dismissed.get(1, TimeUnit.SECONDS))
        service.release.countDown()
        assertEquals(42L, reply.get(3, TimeUnit.SECONDS))
    }
    // The embedded daemon has no reply-timeout control. Inject its wire Error through its own sender, after
    // service entry, rather than faking an adapter exception. Reflection is confined to this library fixture.
    private fun busError(native: NativeDesktopSessionBus, embedded: EmbeddedDBusDaemon, name: String) {
        val connection = NativeDesktopSessionBus::class.java.getDeclaredField("connection").apply { isAccessible = true }.get(native)
            as org.freedesktop.dbus.connections.impl.DBusConnection
        val pending = org.freedesktop.dbus.connections.base.AbstractConnectionBase::class.java.getDeclaredField("pendingCalls")
            .apply { isAccessible = true }.get(connection) as Map<*, *>
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3)
        var call: org.freedesktop.dbus.messages.MethodCall? = null
        while (call == null && System.nanoTime() < deadline) {
            call = synchronized(pending) { pending.values.filterIsInstance<org.freedesktop.dbus.messages.MethodCall>().singleOrNull { it.name == "Notify" } }
            if (call == null) Thread.sleep(5)
        }
        checkNotNull(call) { "Native Notify was not registered" }
        val daemon = EmbeddedDBusDaemon::class.java.getDeclaredField("daemon").apply { isAccessible = true }.get(embedded)
        val names = daemon.javaClass.getDeclaredField("names").apply { isAccessible = true }.get(daemon) as Map<*, *>
        val target = checkNotNull(names[connection.uniqueName])
        val error = connection.messageFactory.createError("org.freedesktop.DBus", connection.uniqueName, "org.freedesktop.DBus.Error.$name",
            call.serial, "s", "Delivery outcome unavailable")
        daemon.javaClass.declaredMethods.single { it.name == "send" && it.parameterCount == 2 }.apply { isAccessible = true }.invoke(daemon, target, error)
    }
    @Test fun busNoReplyDoesNotRollbackOrRetryVisibleEffect() = withBus { native, service, daemon, _ ->
        kotlinx.coroutines.runBlocking {
            service.slow = true
            AuditFinalClientCNotifyRuntime(native).use { f ->
                try {
                    f.start(); f.install()
                    assertTrue(service.entered.await(3, TimeUnit.SECONDS))
                    busError(native, daemon, "NoReply")
                    kotlinx.coroutines.delay(100)
                    assertTrue(f.records().single().inFlight, "Bus NoReply must not perform a definite rollback")
                    kotlinx.coroutines.delay(1_200)
                    f.cache.replaceNotifications(listOf(notificationFixture()))
                    kotlinx.coroutines.delay(100)
                    assertTrue(f.records().single().inFlight, "Bus NoReply must retain ambiguous suppression, not definite rollback")
                    assertEquals(1, service.attempts.get(), "Bus NoReply must not resend the same version after a visible effect")
                } finally { service.release.countDown() }
            }
        }
    }
    @Test fun busTimeoutAndConnectionErrorNamesAreAmbiguous() = withBus { native, service, daemon, _ ->
        service.slow = true
        try {
            for (name in listOf("NoReply", "Timeout", "TimedOut", "Disconnected")) {
                val reply = native.notifyAsync(checkNotNull(native.owner()), 0, "Alert", "Body", emptyList())
                assertTrue(service.entered.await(3, TimeUnit.SECONDS))
                busError(native, daemon, name)
                val failure = assertFailsWith<ExecutionException> { reply.get(3, TimeUnit.SECONDS) }
                assertFalse(failure.cause is DesktopNotifyRejected, "$name is ambiguous regardless of its message text")
            }
        } finally { service.release.countDown() }
    }
    @Test fun busPreDeliveryErrorsAreDefinite() = withBus { native, service, daemon, _ ->
        service.slow = true
        try {
            for (name in listOf("ServiceUnknown", "NameHasNoOwner")) {
                val reply = native.notifyAsync(checkNotNull(native.owner()), 0, "Alert", "Body", emptyList())
                assertTrue(service.entered.await(3, TimeUnit.SECONDS))
                busError(native, daemon, name)
                val failure = assertFailsWith<ExecutionException> { reply.get(3, TimeUnit.SECONDS) }
                assertIs<DesktopNotifyRejected>(failure.cause, "$name proves non-delivery")
            }
        } finally { service.release.countDown() }
    }
    @Test fun repeatedTransportLossReleasesPendingRuntimeOwnershipWithoutResending() = disconnected(false)
    @Test fun repeatedNativeCloseReleasesPendingRuntimeOwnershipWithoutResending() = disconnected(true)
    private fun disconnected(explicitClose: Boolean) = withBus { first, service, embedded, address ->
        kotlinx.coroutines.runBlocking {
            service.slow = true
            var current = first
            AuditFinalClientCNotifyRuntime(first, sessionFactory = { current }).use { f ->
                try {
                    repeat(3) { index ->
                        if (index > 0) current = NativeDesktopSessionBus(address)
                        f.start()
                        kotlinx.coroutines.delay(100)
                        assertEquals(index, service.attempts.get(), "Reconnection to the same daemon must not resend unresolved versions")
                        f.cache.install(emptySnapshot(index.toLong() + 1).copy(notifications = (0..index).map { notificationFixture().copy(id = "alert-$it") }))
                        f.eventually("Real service must receive new alert") { service.attempts.get() == index + 1 }
                        val pending = DesktopLive::class.java.getDeclaredField("pendingNotifies").apply { isAccessible = true }.get(f.live) as Set<*>
                        assertEquals(1, pending.size)
                        if (explicitClose) current.close() else {
                            val connection = NativeDesktopSessionBus::class.java.getDeclaredField("connection").apply { isAccessible = true }.get(current)
                                as org.freedesktop.dbus.connections.impl.DBusConnection
                            val daemon = EmbeddedDBusDaemon::class.java.getDeclaredField("daemon").apply { isAccessible = true }.get(embedded)
                            val names = daemon.javaClass.getDeclaredField("names").apply { isAccessible = true }.get(daemon) as Map<*, *>
                            val target = checkNotNull(names[connection.uniqueName])
                            (target.javaClass.getDeclaredField("connection").apply { isAccessible = true }.get(target) as java.io.Closeable).close()
                        }
                        f.eventually("Disconnected native calls must release all PendingNotify ownership") { pending.isEmpty() }
                        assertTrue(f.records().all { it.inFlight && it.handle == 0L }, "Lost replies remain durably ambiguous")
                        assertEquals(index + 1, service.attempts.get())
                        f.stop()
                    }
                } finally { service.release.countDown() }
            }
        }
    }
    @Test fun realDaemonErrorIsDefiniteRejection() = withBus { native, service, _, _ ->
        service.reject = true
        val reply = native.notifyAsync(checkNotNull(native.owner()), 0, "Alert", "Body", emptyList())
        val failure = assertFailsWith<ExecutionException> { reply.get(3, TimeUnit.SECONDS) }
        assertTrue(service.entered.await(1, TimeUnit.SECONDS), "Error must originate from the exported service")
        assertIs<DesktopNotifyRejected>(failure.cause, "An explicit D-Bus error proves no native effect")
        assertTrue(failure.cause?.message.orEmpty().contains("Service rejected delivery"))
    }
}
