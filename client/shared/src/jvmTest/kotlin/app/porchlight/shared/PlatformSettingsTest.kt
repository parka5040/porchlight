package app.porchlight.shared

import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import java.io.File
import kotlin.test.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.freedesktop.dbus.types.UInt32
import org.freedesktop.dbus.types.Variant

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class PlatformSettingsTest {
    @Test fun portalValuesAndMissingPortalUseDarkFallback() {
        assertTrue(portalPrefersDark(null))
        assertTrue(portalPrefersDark(0))
        assertTrue(portalPrefersDark(1))
        assertFalse(portalPrefersDark(2))
        assertFalse(portalPrefersDark(Variant(Variant(UInt32(2)))))
        assertTrue(portalPrefersDark("invalid"))
    }
    @Test fun portalSubscriptionTracksChangesAndNeverDelaysConstruction() = runTest {
        val events = MutableSharedFlow<Any?>(replay = 1)
        val source = PortalAppearanceSource { events }
        val theme = DesktopPortalTheme(backgroundScope, source, StandardTestDispatcher(testScheduler))
        assertTrue(theme.dark.value)
        events.emit(2)
        runCurrent()
        assertFalse(theme.dark.value)
        events.emit(1)
        runCurrent()
        assertTrue(theme.dark.value)
    }
    @Test fun failedPortalUsesDark() = runTest {
        val theme = DesktopPortalTheme(backgroundScope, PortalAppearanceSource { flow { error("portal unavailable") } },
            StandardTestDispatcher(testScheduler))
        runCurrent()
        assertTrue(theme.dark.value)
    }
    @Test fun dbusWiringAcceptsUniqueSenderAndKeepsSubscriptionAfterReadFailure() = runTest {
        class UniqueSenderBus : PortalTransport {
            var senderFilter: String? = "not-subscribed"
            var handler: ((PortalSettings.SettingChanged) -> Unit)? = null
            var closed = false
            var subscriptionClosed = false
            override fun subscribe(sender: String?, handler: (PortalSettings.SettingChanged) -> Unit): AutoCloseable {
                senderFilter = sender
                this.handler = handler
                return AutoCloseable { subscriptionClosed = true }
            }
            override fun read(): Any? = throw java.io.IOException("appearance key absent")
            override fun close() { closed = true }
            fun signal(namespace: String, key: String, value: Int) {
                // dbus-java matches the signal's unique source, not its service's well-known name.
                if (senderFilter == null || senderFilter == ":1.42") {
                    handler?.invoke(PortalSettings.SettingChanged("/org/freedesktop/portal/desktop", namespace, key, Variant(UInt32(value.toLong()))))
                }
            }
        }
        val bus = UniqueSenderBus()
        val source = DbusPortalAppearanceSource { bus }
        val theme = DesktopPortalTheme(backgroundScope, source, UnconfinedTestDispatcher(testScheduler))
        runCurrent()
        assertNull(bus.senderFilter)
        assertFalse(bus.closed)
        assertFalse(bus.subscriptionClosed)
        bus.signal("unrelated", "color-scheme", 2)
        runCurrent()
        assertTrue(theme.dark.value)
        bus.signal("org.freedesktop.appearance", "color-scheme", 2)
        runCurrent()
        assertFalse(theme.dark.value)
        bus.signal("org.freedesktop.appearance", "color-scheme", 1)
        runCurrent()
        assertTrue(theme.dark.value)
        backgroundScope.coroutineContext[kotlinx.coroutines.Job]!!.cancel()
        runCurrent()
        assertTrue(bus.closed)
        assertTrue(bus.subscriptionClosed)
    }
    @Test fun realDataStoreSurvivesCloseAndReopen() = runTest {
        val directory = java.nio.file.Files.createTempDirectory(File("../../var").toPath(), "c1a-tests-").toFile()
        val file = File(directory, "roundtrip.preferences_pb")
        var job = kotlinx.coroutines.SupervisorJob()
        try {
            fun store(): DataStoreSettingsStore = DataStoreSettingsStore(
                androidx.datastore.preferences.core.PreferenceDataStoreFactory.create(
                    scope = kotlinx.coroutines.CoroutineScope(job + kotlinx.coroutines.Dispatchers.IO), produceFile = { file }))
            val first = store()
            first.saveRoute(app.porchlight.shared.navigation.AppRoute.Lists("groceries"))
            first.setTheme(ThemeMode.Evening)
            job.cancel()
            job.join()
            job = kotlinx.coroutines.SupervisorJob()
            val restored = store().settings.first()
            assertEquals(app.porchlight.shared.navigation.AppRoute.Lists("groceries"), restored.currentRoute)
            assertEquals(ThemeMode.Evening, restored.themeMode)
            job.cancel()
            job.join()
        } finally { job.cancel(); job.join(); directory.deleteRecursively() }
    }
    @Test fun xdgLocationAndVersionAreCorrect() {
        assertEquals(File("/home/test/.local/share/porchlight"), desktopDataDirectory(null, "/home/test"))
        assertEquals(File("/data/porchlight"), desktopDataDirectory("/data", "/home/test"))
        assertEquals(File("/home/test/.local/share/porchlight"), desktopDataDirectory("relative", "/home/test"))
        assertEquals(File("../../VERSION").readText().trim(), BuildInfo.VERSION)
    }
}
