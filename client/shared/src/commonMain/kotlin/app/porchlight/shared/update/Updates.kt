package app.porchlight.shared.update

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.PlatformKind
import app.porchlight.shared.api.models.ClientRelease
import app.porchlight.shared.api.models.ReleaseFile
import app.porchlight.shared.api.models.ServerRelease
import app.porchlight.shared.backups.DownloadHead
import app.porchlight.shared.settings.SettingsStore
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlin.time.Instant
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.contentOrNull

sealed interface UpdateState {
    data object Idle : UpdateState
    data class Available(val release: ClientRelease) : UpdateState
    data class Downloading(val received: Long, val total: Long) : UpdateState
    data object Verifying : UpdateState
    data object NeedsPermission : UpdateState
    data object Installing : UpdateState
    data object ReadyToRestart : UpdateState
    data class Failed(val reason: String) : UpdateState
    data class Unavailable(val reason: String) : UpdateState
}
const val DOWNLOAD_MISMATCH = "The download did not match the release; nothing was installed."
class UpdateFailure(val reason: String) : Exception(reason)

/** Wire models are generated from OpenAPI, including the device-only release endpoints. */
interface UpdateSource {
    suspend fun release(): ClientRelease
    suspend fun server(): ServerRelease? = null
    suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit)
}
class TrustedUpdateSource(private val trust: TrustController) : UpdateSource {
    private suspend fun <T> deviceCall(block: suspend (ApiTransport, () -> Unit) -> T): T {
        val identity = trust.notificationIdentity() ?: throw CancellationException("Device trust unavailable")
        val epoch = trust.sessionEpoch.value
        fun fence() {
            if (!updateTrustUsable(trust.state.value) || identity != trust.notificationIdentity() || epoch != trust.sessionEpoch.value)
                throw CancellationException("Device trust changed")
        }
        fence()
        try { return block(trust.transport(), ::fence).also { fence() } }
        catch (e: DeviceCertificateRevoked) { fence(); trust.onTransportError(e); throw e }
        catch (e: ApiProblem) { fence(); trust.onProblem(e); throw e }
    }
    override suspend fun release(): ClientRelease = deviceCall { api, _ ->
        WireJson.decodeFromJsonElement(ClientRelease.serializer(), api.request("GET", "/v1/client-release"))
    }
    override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) = deviceCall { api, fence ->
        requireReleasePath(file.path)
        api.download("/v1/client-release/files/${file.path}", null, if (offset > 0) "bytes=$offset-" else null,
            "\"${file.sha256}\"", { fence(); begin(it) }, { fence(); chunk(it) })
    }
    override suspend fun server(): ServerRelease? {
        if (trust.state.value != TrustState.Ready) return null
        val epoch = trust.sessionEpoch.value
        val session = trust.sessionToken() ?: return null
        fun current() = trust.state.value == TrustState.Ready && epoch == trust.sessionEpoch.value && session == trust.sessionToken()
        return try {
            val api = trust.transport()
            val me = api.request("GET", "/v1/me", session = session)
            if (!current() || me["role"]?.jsonPrimitive?.contentOrNull != "super_admin") return null
            val result = WireJson.decodeFromJsonElement(ServerRelease.serializer(), api.request("GET", "/v1/server-release", session = session))
            result.takeIf { current() }
        } catch (e: CancellationException) { throw e }
        catch (e: ApiProblem) { if (current()) trust.onProblem(e); null }
        catch (e: DeviceCertificateRevoked) { if (current()) trust.onTransportError(e); null }
        catch (_: Exception) { null }
    }
}

interface UpdatePlatform {
    val kind: PlatformKind
    val events: Flow<UpdateState> get() = emptyFlow()
    suspend fun unavailable(release: ClientRelease): String?
    suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState
    suspend fun resumePermission(report: (UpdateState) -> Unit): UpdateState = UpdateState.NeedsPermission
    fun requestPermission() {}
    suspend fun restart() {}
    fun cancel() {}
}
expect fun platformUpdater(context: Any? = null, launcher: String? = null, quit: () -> Unit = {}): UpdatePlatform

internal fun updateTrustUsable(state: TrustState) = state in setOf(TrustState.Ready, TrustState.PinRequired, TrustState.Login, TrustState.SessionExpired) || state is TrustState.UpgradeRequired
fun releaseVersionCode(version: String): Long? {
    if (!version.matches(Regex("[0-9]+\\.[0-9]+\\.[0-9]+"))) return null
    val parts = version.split('.').map { it.toLongOrNull() ?: return null }
    if (parts[0] > (Int.MAX_VALUE / 10000).toLong() || parts[1] > 99 || parts[2] > 99) return null
    return (parts[0] * 10000 + parts[1] * 100 + parts[2]).takeIf { it in 1..Int.MAX_VALUE }
}
fun requireReleasePath(path: String) {
    require(path.length in 1..512 && path.split('/').all { it.matches(Regex("[A-Za-z0-9_][A-Za-z0-9._-]*")) && it !in setOf(".", "..") }) { "Invalid release path" }
}

/** Application-owned jobs survive rotation/PIN relock; checks cannot replace an in-flight install. */
class UpdateController(private val source: UpdateSource, private val platform: UpdatePlatform,
    private val settings: SettingsStore, private val scope: CoroutineScope, private val now: () -> Instant,
    private val buildCode: Long = BuildInfo.VERSION_CODE.toLong(), private val buildVersion: String = BuildInfo.VERSION) {
    private val mutable = MutableStateFlow<UpdateState>(UpdateState.Idle)
    val state = mutable.asStateFlow()
    private val version = MutableStateFlow<String?>(null)
    val serverVersion = version.asStateFlow()
    private val serverStatus = MutableStateFlow<ServerRelease?>(null)
    val serverRelease = serverStatus.asStateFlow()
    private val checking = Mutex()
    private val generation = MutableStateFlow(0L)
    private val eventRevision = MutableStateFlow(0L)
    init { scope.launch { platform.events.collect { eventRevision.update { n -> n + 1 }; mutable.value = it } } }
    private var boundTrust: TrustController? = null
    private var release: ClientRelease? = null
    private var lastCheck: Instant? = null
    private var operation: Job? = null
    fun check(force: Boolean = true): Job = scope.launch { checkNow(force) }
    suspend fun checkNow(force: Boolean = true) {
        if (boundTrust?.let { !updateTrustUsable(it.state.value) } == true || !checking.tryLock()) return
        val epoch = generation.value
        val event = eventRevision.value
        try {
            if (operation?.isActive == true || state.value in setOf(UpdateState.NeedsPermission, UpdateState.Installing, UpdateState.ReadyToRestart)) return
            val at = now()
            if (!force && lastCheck?.let { (at - it).inWholeSeconds in 0 until 3600 } == true) return
            lastCheck = at
            release = null
            if (buildCode == 0L) { mutable.value = UpdateState.Unavailable("Updates are disabled for this development version."); return }
            try {
                val next = source.release()
                if (epoch != generation.value) return
                version.value = next.version
                val remoteCode = releaseVersionCode(next.version) ?: throw UpdateFailure("Invalid release version; nothing was installed.")
                if (next.android.versionCode != remoteCode) throw UpdateFailure("Invalid release version; nothing was installed.")
                val current = if (platform.kind == PlatformKind.Android) buildCode else releaseVersionCode(buildVersion) ?: 0L
                release = next.takeIf { remoteCode > current }
                val result = if (release == null) UpdateState.Idle else platform.unavailable(next)?.let(UpdateState::Unavailable) ?: UpdateState.Available(next)
                if (epoch == generation.value && event == eventRevision.value) mutable.value = result
            } catch (e: CancellationException) { if (epoch == generation.value) lastCheck = null; throw e }
            catch (e: ApiProblem) {
                if (epoch != generation.value) return
                if (e.status == 404 && e.type == "client-release-unavailable") { release = null; mutable.value = UpdateState.Idle }
                else mutable.value = UpdateState.Failed("Unable to check for updates. Reconnect and try again.")
            } catch (e: UpdateFailure) { if (epoch == generation.value) mutable.value = UpdateState.Failed(e.reason) }
            catch (e: Exception) { app.porchlight.shared.reportDiagnostic("Checking for updates", e); if (epoch == generation.value) mutable.value = UpdateState.Failed("Unable to check for updates. Reconnect and try again.") }
            if (epoch == generation.value) {
                val status = source.server()
                if (epoch == generation.value) serverStatus.value = status
            }
        } finally { checking.unlock() }
    }
    fun update(): Job? {
        val owner = Any()
        if (!checking.tryLock(owner)) return null
        if (operation?.isActive == true || state.value in setOf(UpdateState.Installing, UpdateState.ReadyToRestart, UpdateState.NeedsPermission) || state.value is UpdateState.Unavailable) {
            checking.unlock(owner); return null
        }
        val selected = release ?: run { checking.unlock(owner); return null }
        val epoch = generation.value
        fun report(value: UpdateState) { if (epoch == generation.value) mutable.value = value }
        val event = eventRevision.value
        val job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                platform.unavailable(selected)?.let { report(UpdateState.Unavailable(it)); return@launch }
                val result = platform.install(selected, source, ::report)
                if (event == eventRevision.value) report(result)
            } catch (e: CancellationException) { report(UpdateState.Available(selected)); throw e }
            catch (e: UpdateFailure) { report(UpdateState.Failed(e.reason)) }
            catch (e: Exception) { app.porchlight.shared.reportDiagnostic("Installing the update", e); report(UpdateState.Failed("Update failed. Check your connection and storage, then retry.")) }
        }
        operation = job
        job.invokeOnCompletion { checking.unlock(owner) }
        return job.also { it.start() }
    }
    fun retry() = scope.launch {
        if (release == null) checkNow()
        update()
    }
    fun onResume() {
        if (state.value != UpdateState.NeedsPermission) { check(false); return }
        val owner = Any()
        if (!checking.tryLock(owner)) return
        val epoch = generation.value
        val event = eventRevision.value
        fun report(value: UpdateState) { if (epoch == generation.value) mutable.value = value }
        val job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                val result = platform.resumePermission(::report)
                if (event == eventRevision.value) report(result)
            } catch (e: CancellationException) { throw e }
            catch (e: UpdateFailure) { report(UpdateState.Failed(e.reason)) }
            catch (e: Exception) { app.porchlight.shared.reportDiagnostic("Resuming the update", e); report(UpdateState.Failed("Unable to start the Android installer. Try again.")) }
        }
        operation = job
        job.invokeOnCompletion { checking.unlock(owner) }
        job.start()
    }
    fun requestPermission() {
        try { platform.requestPermission() }
        catch (_: Exception) { mutable.value = UpdateState.Failed("Unable to open install permission settings. Try again.") }
    }
    fun restart() = scope.launch {
        try { platform.restart() }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { mutable.value = UpdateState.Failed("Unable to restart. Close Porchlight and open it again.") }
    }
    fun cancel() {
        if (state.value == UpdateState.Installing) return // the OS/atomic installer owns this phase
        operation?.cancel(); platform.cancel(); release?.let { mutable.value = UpdateState.Available(it) }
    }
    fun dismiss(version: String) = scope.launch {
        try { settings.dismissUpdate(version) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { mutable.value = UpdateState.Failed("The update reminder could not be saved. Try again.") }
    }
    fun clearServerStatus() { serverStatus.value = null }
    fun reset() { generation.update { it + 1 }; operation?.cancel(); platform.cancel(); release = null; lastCheck = null; version.value = null; serverStatus.value = null; mutable.value = UpdateState.Idle }
    fun bind(trust: TrustController, live: app.porchlight.shared.live.LiveSync? = null) {
        if (boundTrust != null) return
        boundTrust = trust
        scope.launch {
            var previousEpoch = trust.sessionEpoch.value
            var previousState: TrustState? = null
            combine(trust.state, trust.sessionEpoch) { state, epoch -> state to epoch }.collect { (state, epoch) ->
                if (epoch != previousEpoch) { reset(); previousEpoch = epoch }
                if (!updateTrustUsable(state)) reset() else {
                    if (state != TrustState.Ready) clearServerStatus()
                    check(false)
                    if (state == TrustState.Ready && previousState != TrustState.Ready) scope.launch {
                        val value = source.server()
                        if (trust.state.value == TrustState.Ready && trust.sessionEpoch.value == epoch) serverStatus.value = value
                    }
                }
                previousState = state
            }
        }
        if (live != null) scope.launch { live.state.collect { if (it == app.porchlight.shared.live.ConnectionState.Connected) check() } }
    }
}
