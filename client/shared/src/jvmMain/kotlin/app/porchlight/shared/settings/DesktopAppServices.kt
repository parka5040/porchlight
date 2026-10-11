package app.porchlight.shared.settings

import androidx.datastore.core.handlers.ReplaceFileCorruptionHandler
import androidx.datastore.preferences.core.*
import app.porchlight.shared.*
import app.porchlight.shared.theme.DesktopPortalTheme
import java.io.File
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.collectLatest
import kotlin.time.Clock
import app.porchlight.shared.backups.registerBackupStatus

fun desktopDataDirectory(xdgDataHome: String?, userHome: String): File {
    val base = xdgDataHome?.takeIf { it.isNotBlank() }?.let(::File)?.takeIf { it.isAbsolute }
        ?: File(userHome, ".local/share")
    return File(base, "porchlight")
}

/** One instance per application, shared across all windows. close() on application shutdown. */
class DesktopAppServices : AppServices, AutoCloseable {
    override val pendingOpen = kotlinx.coroutines.flow.MutableStateFlow<app.porchlight.shared.navigation.AppRoute?>(null)
    val trayAvailable = kotlinx.coroutines.flow.MutableStateFlow<Boolean?>(null)
    override val desktopLoginConflict = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
    override val dismissDesktopLoginConflict: () -> Unit = { desktopLoginConflict.value = null }
    override val runtimeNotice = kotlinx.coroutines.flow.MutableStateFlow<String?>(null)
    override val systemResumes = kotlinx.coroutines.flow.MutableStateFlow(0L)
    private var liveRuntime: app.porchlight.shared.live.DesktopLive? = null
    private val liveStarted = CompletableDeferred<app.porchlight.shared.live.DesktopLive>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val directory = desktopDataDirectory(System.getenv("XDG_DATA_HOME"), System.getProperty("user.home"))
    override val settings: SettingsStore = DataStoreSettingsStore(PreferenceDataStoreFactory.create(
        corruptionHandler = ReplaceFileCorruptionHandler { emptyPreferences() }, scope = scope,
        produceFile = { File(directory, "device.preferences_pb") },
    ))
    override suspend fun stepUp(password: String, action: String): String = trust.controller.reauth(password, action)
    override val clock = AppClock { Clock.System.now() }
    override val screenSession = app.porchlight.shared.screens.ScreenSession()
    override val platform = object : PlatformInfo {
        override val kind = PlatformKind.Desktop
        override val systemDark = DesktopPortalTheme(scope).dark
    }
    // c2b: the same root-only mTLS coordinator is shared by Account and generated repositories.
    val trust = app.porchlight.shared.trust.AppTrustHolder(
        { app.porchlight.shared.trust.DesktopKeyStore() },
        { keys -> app.porchlight.shared.trust.TrustController(keys,
            app.porchlight.shared.transport.OkHttpTrustFactory(keys, app.porchlight.shared.transport.ClientInfo(BuildInfo.VERSION)),
            "desktop", { clock.now() }, pinRequired = false) })
    private val passivePull = app.porchlight.shared.backups.PassivePull(
        File(System.getProperty("user.home"), ".local/share/porchlight/backups").toPath(),
        app.porchlight.shared.backups.TrustedBackupSource(trust.controller), { clock.now() },
    ).also { pull ->
        pull.start(scope)
        scope.launch {
            kotlinx.coroutines.flow.combine(trust.controller.state, trust.controller.sessionEpoch) { state, epoch -> state to epoch }
                .collectLatest { (state, _) ->
                    pull.resetView()
                    if (state == app.porchlight.shared.trust.TrustState.Ready) {
                        try { pull.tick() } catch (_: CancellationException) { currentCoroutineContext().ensureActive() }
                    }
                }
        }
    }
    override val backups: app.porchlight.shared.backups.DesktopBackups get() = passivePull
    private val lazyData = lazy {
        val file = File(directory, "read-cache.db")
        app.porchlight.shared.data.HouseholdData(
            // The cache opens (mkdirs, PRAGMA check, possible recreate) on its first use inside initialize(), which
            // runs on Dispatchers.Default — never during composition, where this lazy value is first read.
            app.porchlight.shared.data.desktopReadCache(file, app.porchlight.shared.data.FileAttachmentStore(File(System.getenv("XDG_CACHE_HOME")?.takeIf { File(it).isAbsolute } ?: File(System.getProperty("user.home"), ".cache").path, "porchlight/attachments"))) { clock.now() },
            app.porchlight.shared.data.GeneratedCall.trusted(trust.controller), { clock.now() },
        ).also { it.bindTrust(trust.controller, scope, screenSession::bind,
            onIdentityEnded = { epoch -> liveStarted.await().clearIdentity(epoch) },
            onIdentityStarting = { owner, epoch -> liveStarted.await().prepareIdentity(owner, epoch) }) }
    }
    override val data get() = lazyData.value
    override val settingsWrites by lazy { app.porchlight.shared.screens.settings.HouseholdSettingsController(data) }
    private val launcher = app.porchlight.shared.live.resolveDesktopLauncher(
        System.getProperty("jpackage.app-path") ?: System.getProperty("porchlight.launcher"),
        ProcessHandle.current().info().command().orElse(null),
        directory.toPath().resolve("current/app/bin/porchlight"))
    private var quitAfterUpdate: () -> Unit = {}
    override val updates by lazy {
        app.porchlight.shared.update.UpdateController(app.porchlight.shared.update.TrustedUpdateSource(trust.controller),
            app.porchlight.shared.update.platformUpdater(launcher = launcher, quit = { quitAfterUpdate() }), settings, scope, { clock.now() })
            .also { updater ->
                updater.bind(trust.controller, data.live)
                scope.launch { systemResumes.collect { updater.onResume() } }
            }
    }
    override val desktopLoginHelp: String = launcher?.let {
        "Start at login uses one XDG autostart entry. For niri instead, turn this switch OFF and add only this line to config.kdl:\n" + app.porchlight.shared.live.niriAutostart(it)
    } ?: "Start at login is available in the installed packaged app (not Gradle)."
    val trayActions = app.porchlight.shared.live.DesktopTrayActions()
    fun startLive(open: () -> Unit, quit: () -> Unit) {
        quitAfterUpdate = quit
        if (liveRuntime != null) return
        trayActions.registerBackupStatus(backups.state.value)
        scope.launch { backups.state.collect { trayActions.registerBackupStatus(it) } }
        liveRuntime = app.porchlight.shared.live.DesktopLive(data, directory.toPath(), scope, { clock.now() }, { item, occurrence ->
            if (item != null) pendingOpen.value = app.porchlight.shared.navigation.AppRoute.Item(item, occurrence)
            open()
        }, quit, runtimeNotice, trust.controller, trayActions, trayAvailable = trayAvailable, systemResumes = systemResumes)
        liveStarted.complete(checkNotNull(liveRuntime))
        scope.launch {
            settings.settings.map { it.stayConnected }.distinctUntilChanged().collect { enabled ->
                val path = launcher
                if (enabled && path == null) runtimeNotice.value = "Start at login is unavailable in a Gradle run. Install the packaged app first."
                else if (path != null) try {
                    applyDesktopLoginSetting(java.nio.file.Path.of(System.getenv("XDG_CONFIG_HOME") ?: (System.getProperty("user.home") + "/.config")), path, enabled, settings, desktopLoginConflict)
                } catch (_: Exception) { runtimeNotice.value = "Start at login could not be changed. If niri already starts Porchlight, leave this switch off or remove that line first; otherwise check XDG config permissions." }
            }
        }
    }
    override fun close() {
        // Drain native replies to disk before cancelling completion/binding work or closing the read cache.
        liveRuntime?.close()
        scope.cancel()
        if (lazyData.isInitialized()) lazyData.value.close()
    }
}

/** The service's settings collector and host tests share the same persisted niri-conflict policy. */
internal suspend fun applyDesktopLoginSetting(configHome: java.nio.file.Path, launcher: String, enabled: Boolean,
    settings: SettingsStore, conflict: kotlinx.coroutines.flow.MutableStateFlow<String?>) {
    if (app.porchlight.shared.live.writeDesktopAutostart(configHome, launcher, enabled) == app.porchlight.shared.live.DesktopAutostartResult.NiriManaged) {
        settings.setStayConnected(false)
        conflict.value = "Niri already starts Porchlight. Start at login was turned off to avoid a duplicate XDG entry."
    }
}
