package app.porchlight.shared.update

import android.app.PendingIntent
import android.content.*
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import app.porchlight.shared.PlatformKind
import app.porchlight.shared.api.models.ClientRelease
import app.porchlight.shared.api.models.ReleaseFile
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.emitAll
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest

actual fun platformUpdater(context: Any?, launcher: String?, quit: () -> Unit): UpdatePlatform = AndroidUpdater.get(context as Context)
internal fun privateUpdateDirectory(path: Path): Path {
    Files.createDirectories(path, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
    require(Files.isDirectory(path, LinkOption.NOFOLLOW_LINKS))
    Files.setPosixFilePermissions(path, PosixFilePermissions.fromString("rwx------"))
    return path
}
@Suppress("DEPRECATION")
private fun PackageInfo.identity() = ApkIdentity(packageName, longVersionCode,
    signingInfo?.apkContentsSigners?.map { digestHex(MessageDigest.getInstance("SHA-256").digest(it.toByteArray())) }?.toSet())

class AndroidUpdater private constructor(private val context: Context) : UpdatePlatform {
    override val kind = PlatformKind.Android
    private val manager get() = context.packageManager
    private val root get() = realUpdateRoot(context.noBackupFilesDir.toPath().resolve("updates"))
    private val prefs = context.getSharedPreferences("update-install", Context.MODE_PRIVATE)
    private val outcomes = MutableSharedFlow<UpdateState>(replay = 1, extraBufferCapacity = 4)
    override val events = flow {
        withContext(Dispatchers.IO) {
            val id = prefs.getInt("session", -1)
            if (id >= 0) {
                val target = prefs.getString("version", null)?.let(::releaseVersionCode) ?: Long.MAX_VALUE
                val restored = restoredInstallState(installed().versionCode, target, manager.packageInstaller.getSessionInfo(id) != null)
                if (restored != UpdateState.Installing) cleanup()
                outcomes.emit(restored)
            } else prefs.getString("failure", null)?.let { outcomes.emit(UpdateState.Failed(it)) }
        }
        emitAll(outcomes)
    }
    private var pending: Pair<ClientRelease, Path>? = null
    @Suppress("DEPRECATION")
    private fun installed() = manager.getPackageInfo(context.packageName, PackageManager.GET_SIGNING_CERTIFICATES).identity()
    private fun advertised(release: ClientRelease) = release.android.let { ApkIdentity(it.`package`, it.versionCode, it.signerSha256.toSet()) }
    override suspend fun unavailable(release: ClientRelease): String? = withContext(Dispatchers.IO) { apkPreflight(installed(), advertised(release)) }
    override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState = withContext(Dispatchers.IO) {
        apkPreflight(installed(), advertised(release))?.let { throw UpdateFailure(it) }
        prefs.edit().remove("failure").commit()
        require(releaseVersionCode(release.version) != null)
        val directory = guardedUpdateDirectory(root)
        Files.newDirectoryStream(directory).use { entries -> entries.filter { it.fileName.toString() != release.version }.forEach(::removeUpdateTree) }
        val dir = guardedUpdateDirectory(directory.resolve(release.version))
        val apk = release.android
        report(UpdateState.Downloading(0, apk.propertySize))
        val payload = downloadReleaseFiles(source, listOf(ReleaseFile(apk.file, apk.propertySize, apk.sha256)), dir, report)
        pending = release to updatePath(payload, apk.file)
        continueInstall(report)
    }
    @Suppress("DEPRECATION")
    private suspend fun continueInstall(report: (UpdateState) -> Unit): UpdateState {
        val (release, path) = pending ?: throw UpdateFailure("The update file is unavailable. Download it again.")
        try {
            report(UpdateState.Verifying)
            if (!Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) || Files.size(path) != release.android.propertySize || fileDigest(path) != release.android.sha256)
                throw UpdateFailure(DOWNLOAD_MISMATCH)
            val archive = manager.getPackageArchiveInfo(path.toString(), PackageManager.GET_SIGNING_CERTIFICATES)?.identity()
            apkDecision(installed(), advertised(release), archive)?.let { throw UpdateFailure(it) }
            if (!manager.canRequestPackageInstalls()) return UpdateState.NeedsPermission
            currentCoroutineContext().ensureActive()
            report(UpdateState.Installing)
            val installer = manager.packageInstaller
            val parameters = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
                setAppPackageName(context.packageName)
                setSize(release.android.propertySize)
            }
            val id = installer.createSession(parameters)
            try {
                installer.openSession(id).use { session ->
                    session.openWrite("base.apk", 0, release.android.propertySize).use { out ->
                        Files.newInputStream(path, LinkOption.NOFOLLOW_LINKS).use { it.copyTo(out) }; session.fsync(out)
                    }
                    check(prefs.edit().putInt("session", id).putString("version", release.version).commit())
                    val callback = Intent(context, UpdateInstallReceiver::class.java).setPackage(context.packageName)
                        .setAction("app.porchlight.UPDATE_INSTALL.$id")
                    val result = PendingIntent.getBroadcast(context, id, callback,
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE)
                    session.commit(result.intentSender)
                }
            } catch (e: Exception) { installer.abandonSession(id); throw e }
            return UpdateState.Installing
        } catch (e: Exception) { cleanup(); throw e }
    }
    override suspend fun resumePermission(report: (UpdateState) -> Unit): UpdateState = withContext(Dispatchers.IO) {
        if (!manager.canRequestPackageInstalls()) UpdateState.NeedsPermission else continueInstall(report)
    }
    override fun requestPermission() {
        context.startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
    private fun cleanup() {
        val version = prefs.getString("version", null) ?: pending?.first?.version
        try { if (version != null && releaseVersionCode(version) != null) removeUpdateTree(root.resolve(version)) }
        catch (_: java.io.IOException) { /* A completed installer outcome must not be masked by stale-file cleanup. */ }
        prefs.edit().clear().commit(); pending = null
    }
    internal suspend fun result(intent: Intent) = withContext(Dispatchers.IO) {
        val id = prefs.getInt("session", -1)
        if (id < 0 || intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -2) != id || intent.action != "app.porchlight.UPDATE_INSTALL.$id") return@withContext
        when (val result = installResult(intent.getIntExtra(PackageInstaller.EXTRA_STATUS, Int.MIN_VALUE), intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE))) {
            InstallResult.Confirm -> {
                @Suppress("DEPRECATION") val confirmation = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                try {
                    checkNotNull(confirmation)
                    context.startActivity(confirmation.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } catch (_: Exception) { recordFailure("Android could not open the install confirmation. Try again.") }
            }
            InstallResult.Success -> { cleanup(); outcomes.emit(UpdateState.Idle) }
            is InstallResult.Failure -> recordFailure(result.reason)
        }
    }
    private suspend fun recordFailure(reason: String) {
        cleanup()
        prefs.edit().putString("failure", reason).commit()
        outcomes.emit(UpdateState.Failed(reason))
    }
    override fun cancel() {
        // Only the pre-session permission wait is cancellable here; Android owns committed sessions.
        if (prefs.getInt("session", -1) < 0) pending = null
    }
    companion object {
        @Volatile private var instance: AndroidUpdater? = null
        fun get(context: Context): AndroidUpdater = instance ?: synchronized(this) {
            instance ?: AndroidUpdater(context.applicationContext).also { instance = it }
        }
    }
}
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            try { AndroidUpdater.get(context).result(intent) } finally { pending?.finish() }
        }
    }
}
