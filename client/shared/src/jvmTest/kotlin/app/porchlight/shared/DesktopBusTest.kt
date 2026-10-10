package app.porchlight.shared

import app.porchlight.shared.backups.registerBackupStatus

import app.porchlight.shared.live.*
import org.freedesktop.dbus.Marshalling
import org.freedesktop.dbus.interfaces.DBusInterface
import org.freedesktop.dbus.interfaces.Properties
import org.freedesktop.dbus.messages.DBusSignal
import org.freedesktop.dbus.types.*
import kotlin.test.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.advanceTimeBy

@org.freedesktop.dbus.annotations.DBusInterfaceName("app.porchlight.TestMenuWire")
interface MenuTestWire : DBusInterface {
    class Layout(path: String, val node: MenuNode) : DBusSignal(path, node)
}

/** In-process session bus: exports the production objects, records their real D-Bus signals. */
internal class FakeSessionBus : DesktopSessionBus {
    val exports = mutableMapOf<String, DBusInterface>()
    val signals = mutableListOf<DBusSignal>()
    var owner: String? = ":1.10"
    var listener: DesktopBusEvents? = null
    var registrations = 0
    var watcher = true
    var nextId = 1L
    val replacements = mutableListOf<Long>()
    val closed = mutableListOf<Long>()
    val posts = kotlinx.coroutines.flow.MutableStateFlow(0)
    var ownerCalls = 0
    override fun owner(): String? { ownerCalls++; return owner }
    override fun export(path: String, value: DBusInterface) {
        org.freedesktop.dbus.messages.ExportedObject(value, false); exports[path] = value
    }
    override fun emit(signal: DBusSignal) { signal.appendbody(null); signals += signal }
    override fun watch(events: DesktopBusEvents) { listener = events }
    override fun registerTray() { check(watcher); registrations++ }
    override fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>): Long {
        check(owner == this.owner); replacements += replaces; posts.value++; return if (replaces != 0L) replaces else nextId++
    }
    override fun dismiss(owner: String, id: Long) { check(owner == this.owner); closed += id }
    override fun close() {}
    fun restart(value: String?) { owner = value; nextId = 1; listener!!.ownerChanged(value) }
    fun action(sender: String, id: Long, key: String) { listener!!.action(sender, id, key) }
}

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class DesktopBusTest {
    @Test fun runtimeRestartReplaysDeliveryAndActionsKeepTheOriginalDurableKey() = kotlinx.coroutines.test.runTest {
        val now = kotlin.time.Instant.parse("2026-10-06T12:00:00Z")
        val cache = app.porchlight.shared.data.memoryCache { now }
        val dir = java.nio.file.Files.createTempDirectory(java.nio.file.Path.of(System.getProperty("porchlight.test.dir")), "bus-runtime-")
        val fake = FakeSessionBus()
        val attempts = mutableListOf<String?>()
        val completed = kotlinx.coroutines.CompletableDeferred<Unit>()
        val transport = object : app.porchlight.shared.transport.ApiTransport {
            override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): kotlinx.serialization.json.JsonObject {
                attempts += idempotencyKey
                if (attempts.size == 2) completed.complete(Unit)
                throw app.porchlight.shared.transport.ConnectFailure()
            }
        }
        val data = app.porchlight.shared.data.HouseholdData(cache, app.porchlight.shared.data.GeneratedCall({ transport }, { "session" }), { now })
        var runtime: DesktopLive? = null
        try {
            val row = notificationFixture()
            cache.install(app.porchlight.shared.data.emptySnapshot(1).copy(notifications = listOf(row)))
            val notice = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
            runtime = DesktopLive(data, dir, backgroundScope, { now }, { _, _ -> }, {}, notice, sleepBusFactory = { NoSleepBus }, sessionFactory = { fake })
            fake.posts.first { it == 1 }
            runCurrent()
            val record = app.porchlight.shared.transport.WireJson.parseToJsonElement(java.nio.file.Files.readString(dir.resolve("os-delivery.json"))).jsonObject["records"]!!.jsonArray.single().jsonObject
            val original = record["actionKey"]!!.jsonPrimitive.content
            fake.restart(":1.20")
            fake.posts.first { it == 2 }; runCurrent()
            fake.action(":1.20", 1, "read"); runCurrent()
            fake.action(":1.20", 1, "read"); completed.await()
            assertEquals(listOf<String?>(original, original), attempts.toList())
            cache.replaceNotifications(listOf(row.copy(readAt = now)))
            runCurrent()
            assertEquals(listOf(1L), fake.closed)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun runtimeNoSessionBusRecoversWithoutUnlockingOrLosingPendingDelivery() = kotlinx.coroutines.test.runTest {
        val base = kotlin.time.Instant.parse("2026-10-06T12:00:00Z")
        val now = { base + kotlin.time.Duration.parse("${testScheduler.currentTime}ms") }
        val cache = app.porchlight.shared.data.memoryCache(now)
        val dir = java.nio.file.Files.createTempDirectory(java.nio.file.Path.of(System.getProperty("porchlight.test.dir")), "bus-fallback-")
        val data = app.porchlight.shared.data.HouseholdData(cache, app.porchlight.shared.data.GeneratedCall({ error("No network in test") }, { null }), now)
        val fake = FakeSessionBus(); var available = false
        val notice = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
        var runtime: DesktopLive? = null
        try {
            cache.install(app.porchlight.shared.data.emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
            runtime = DesktopLive(data, dir, backgroundScope, now, { _, _ -> }, {}, notice,
                sleepBusFactory = { NoSleepBus }, sessionFactory = { if (available) fake else error("No bus") })
            notice.first { it != null }
            assertTrue(notice.value!!.contains("Inbox"))
            runCurrent()
            available = true
            advanceTimeBy(15_001)
            fake.posts.first { it == 1 }; runCurrent()
            assertNull(notice.value)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun disconnectedBusUsesSignalThenAbsentOnlyRetry() = kotlinx.coroutines.test.runTest {
        val base = kotlin.time.Instant.parse("2026-10-06T12:00:00Z")
        val now = { base + kotlin.time.Duration.parse("${testScheduler.currentTime}ms") }
        val cache = app.porchlight.shared.data.memoryCache(now)
        val dir = java.nio.file.Files.createTempDirectory(java.nio.file.Path.of(System.getProperty("porchlight.test.dir")), "bus-disconnect-")
        val data = app.porchlight.shared.data.HouseholdData(cache, app.porchlight.shared.data.GeneratedCall({ error("No network") }, { null }), now)
        val first = FakeSessionBus(); val second = FakeSessionBus(); var connects = 0
        var runtime: DesktopLive? = null
        try {
            cache.install(app.porchlight.shared.data.emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
            runtime = DesktopLive(data, dir, backgroundScope, now, { _, _ -> }, {}, kotlinx.coroutines.flow.MutableStateFlow(null),
                sleepBusFactory = { NoSleepBus }, sessionFactory = { if (++connects == 1) first else second })
            first.posts.first { it == 1 }; runCurrent()
            first.listener!!.disconnected(); runCurrent()
            advanceTimeBy(15_001); runCurrent()
            assertEquals(2, connects); assertEquals(1, second.posts.value)
            val calls = second.ownerCalls; advanceTimeBy(60_000); runCurrent()
            assertEquals(calls, second.ownerCalls)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun backupStatusContributionUsesTheLiveRegistryAndOnlyOneSni() {
        val fake = FakeSessionBus()
        val contributions = DesktopTrayActions()
        var opened = 0; var quit = 0
        val actions = app.porchlight.shared.backups.desktopTrayActions(app.porchlight.shared.backups.BackupView(), { opened++ }, { quit++ })
        val bus = DesktopBus(actions.open.invoke, actions.quit.invoke, { _, _ -> }, session = fake, trayActions = contributions)
        try {
            val menu = fake.exports.getValue("/Menu") as TrayMenu
            fun rows() = menu.GetLayout(0, -1, emptyList()).layout.children.map { it.value as MenuNode }
            contributions.registerBackupStatus(app.porchlight.shared.backups.BackupView(allowed = true, pulling = true))
            assertEquals(listOf("Open Porchlight", "Backups: pulling…", "Quit"), rows().map { it.properties["label"]?.value })
            val status = rows()[1]
            assertEquals(false, status.properties["enabled"]?.value)
            contributions.registerBackupStatus(app.porchlight.shared.backups.BackupView(allowed = true, localBundle = "/private/bundle.tar.age"))
            assertEquals("Backups: local bundle verified", rows()[1].properties["label"]?.value)
            assertEquals(status.id, rows()[1].id)
            contributions.registerBackupStatus(app.porchlight.shared.backups.BackupView())
            assertEquals(2, rows().size)
            menu.Event(1, "clicked", Variant(0), UInt32(0)); menu.Event(2, "clicked", Variant(0), UInt32(0))
            assertEquals(1, opened); assertEquals(1, quit)
            assertEquals(1, fake.registrations)
        } finally { bus.close() }
    }
    @Test fun menuSignaturesLayoutsFilteringGroupsAndContributionUseOneTray() {
        val fake = FakeSessionBus(); val contributions = DesktopTrayActions()
        var opened = 0; var quit = 0; var backup = 0
        val bus = DesktopBus({ opened++ }, { quit++ }, { _, _ -> }, session = fake, trayActions = contributions)
        val item = fake.exports.getValue("/StatusNotifierItem") as Properties
        val menu = fake.exports.getValue("/Menu") as TrayMenu
        assertEquals(1, fake.registrations)
        val exported = org.freedesktop.dbus.messages.ExportedObject(menu, false)
        assertTrue(exported.introspectiondata.contains("type=\"(ia{sv}av)\" direction=\"out\""))
        assertEquals("u(ia{sv}av)", Marshalling.getDBusType(MenuLayout::class.java.declaredFields.filter { it.isAnnotationPresent(org.freedesktop.dbus.annotations.Position::class.java) }.map { it.genericType }.toTypedArray()))
        org.freedesktop.dbus.messages.ExportedObject(fake.exports.getValue("/StatusNotifierItem"), false)
        val root = menu.GetLayout(0, -1, emptyList()).layout
        assertEquals(2, root.children.size)
        assertEquals("(ia{sv}av)", root.children.first().sig)
        val wire = MenuTestWire.Layout("/Menu", root)
        wire.appendbody(null) // Exercise dbus-java's real byte marshaller, including empty nested arrays/maps.
        assertEquals("(ia{sv}av)", wire.sig)
        assertTrue(wire.wireData.any { it != null && it.isNotEmpty() })
        assertEquals(0, menu.GetLayout(0, 0, emptyList()).layout.children.size)
        assertEquals(setOf("label"), menu.GetLayout(1, -1, listOf("label")).layout.properties.keys)
        assertFails { menu.GetLayout(99, -1, emptyList()) }
        contributions.register("backup", "Backup status") { backup++ }
        val extra = menu.GetLayout(0, 1, emptyList()).layout.children.map { it.value as MenuNode }.single { it.properties["label"]?.value == "Backup status" }
        menu.Event(extra.id, "clicked", Variant(0), UInt32(0)); assertEquals(1, backup)
        menu.Event(1, "clicked", Variant(0), UInt32(0)); menu.Event(2, "clicked", Variant(0), UInt32(0))
        assertEquals(1, opened); assertEquals(1, quit)
        assertEquals(4, menu.GetGroupProperties(emptyList(), listOf("label")).size)
        assertEquals(listOf(99), menu.EventGroup(listOf(MenuEvent(99, "clicked", Variant(0), UInt32(0)))))
        assertEquals(listOf(99), menu.AboutToShowGroup(listOf(1, 99)).errors.toList())
        bus.updateStatus(true, 0)
        assertEquals("NeedsAttention", item.Get<String>("org.kde.StatusNotifierItem", "Status"))
        assertTrue(fake.signals.any { it is TrayItem.NewStatus })
        contributions.unregister("backup"); assertEquals(2, menu.GetLayout(0, -1, emptyList()).layout.children.size)
        assertEquals(1, fake.registrations); bus.close()
    }
    @Test fun serverReplacementFencesOldSignalsHandlesAndReplaysOriginalActions() {
        val fake = FakeSessionBus(); val actions = mutableListOf<Pair<String, String>>(); val owners = mutableListOf<String?>()
        val bus = DesktopBus({}, {}, { id, key -> actions += id to key }, session = fake, serverChanged = { owners += it })
        val first = bus.notify("row", "Title", "Body", listOf("complete", "Complete"))
        fake.action(":1.10", first, "complete"); fake.action(":1.10", first, "complete")
        assertEquals(listOf("row" to "complete", "row" to "complete"), actions) // repository receives the same durable key on replay
        fake.action(":1.10", first, "invented"); assertEquals(2, actions.size)
        fake.restart(null); assertFails { bus.notify("row", "Title", "Body") }
        fake.restart(":1.20")
        val replacement = bus.notify("row", "Title", "Body", listOf("complete", "Complete"))
        assertEquals(listOf(0L, 0L), fake.replacements)
        fake.action(":1.10", replacement, "complete"); assertEquals(2, actions.size)
        fake.action(":1.20", replacement, "complete"); assertEquals(3, actions.size)
        assertEquals(listOf(null, ":1.20"), owners)
        fake.restart(":1.30"); bus.dismiss("row"); assertTrue(fake.closed.isEmpty())
        bus.close()
    }
    @Test fun watcherReplacementReRegistersOneExportAndRestoredHandlesReplayActions() {
        val fake = FakeSessionBus(); val actions = mutableListOf<Pair<String, String>>(); var degraded = 0
        val bus = DesktopBus({}, {}, { id, action -> actions += id to action }, session = fake, degraded = { degraded++ })
        bus.restoreHandles(listOf(DeliveryRecord("original", 1, posted = true, handle = 42, actionKey = "original-key")))
        fake.action(":1.10", 42, "read"); assertEquals(listOf("original" to "read"), actions)
        fake.watcher = false; fake.listener!!.watcherChanged()
        assertFalse(bus.trayAvailable); assertEquals(1, degraded)
        fake.watcher = true; fake.listener!!.watcherChanged()
        assertTrue(bus.trayAvailable); assertEquals(2, fake.registrations); assertEquals(2, fake.exports.size)
        fake.listener!!.closed(":1.10", 42, 2)
        fake.action(":1.10", 42, "read"); assertEquals(1, actions.size)
        bus.close()
    }
    @Test fun noNotificationServerStillExportsTrayAndNoSessionBusFailsCleanly() {
        val fake = FakeSessionBus().also { it.owner = null }
        val bus = DesktopBus({}, {}, { _, _ -> }, session = fake)
        assertEquals(2, fake.exports.size); assertNull(bus.notificationServerOwner)
        assertFails { bus.notify("row", "Title", "Body") }; bus.close()
        assertNull(desktopBusOrNull { throw IllegalStateException("No bus") })
    }
}
