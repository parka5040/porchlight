package app.porchlight.shared.update

import app.porchlight.shared.PlatformKind
import app.porchlight.shared.api.models.ClientRelease
import app.porchlight.shared.files.ensurePrivateDesktopDirectory
import kotlinx.coroutines.*
import java.nio.file.*

actual fun platformUpdater(context: Any?, launcher: String?, quit: () -> Unit): UpdatePlatform = DesktopUpdater(launcher,
    realUpdateRoot(desktopUpdateDirectory(System.getenv("XDG_STATE_HOME"), System.getProperty("user.home"))), quit = quit)
internal fun privateUpdateDirectory(path: Path): Path = ensurePrivateDesktopDirectory(path)

fun desktopUpdateDirectory(stateHome: String?, home: String): Path =
    (stateHome?.takeIf { it.isNotBlank() }?.let(Path::of)?.takeIf { it.isAbsolute } ?: Path.of(home, ".local/state"))
        .resolve("porchlight/updates")

/** Only a launcher belonging to this installed current image can choose the trusted installer. */
fun desktopInstallBase(launcher: String?): Path? = runCatching {
    val path = launcher?.let(Path::of)?.takeIf { it.isAbsolute }?.normalize() ?: return null
    if (!path.endsWith("app/bin/porchlight")) return null
    val image = path.parent.parent.parent
    val base = when {
        image.fileName.toString() == "current" -> image.parent
        image.parent?.fileName?.toString() == "releases" -> image.parent.parent
        else -> return null
    }
    if (!Files.isRegularFile(base.resolve("current/install.sh")) || !Files.isSameFile(path, base.resolve("current/app/bin/porchlight"))) return null
    base
}.getOrNull()
fun desktopInstallerArgv(base: Path, source: Path) = listOf("bash", base.resolve("current/install.sh").toString(), "desktop", "--source", source.toString())
fun desktopRestartArgv(base: Path) = listOf(base.resolve("current/app/bin/porchlight").toString(), "--after-update")
internal fun toolOnPath(tool: String): Boolean = System.getenv("PATH").orEmpty().split(java.io.File.pathSeparator)
    .filter { it.isNotBlank() }.any { Files.isRegularFile(Path.of(it, tool)) && Files.isExecutable(Path.of(it, tool)) }

class DesktopUpdater(private val launcher: String?, private val root: Path, private val hasTool: (String) -> Boolean = ::toolOnPath,
    private val quit: () -> Unit = {}, private val start: (List<String>, Path?) -> Process = { argv, log ->
        ProcessBuilder(argv).redirectInput(ProcessBuilder.Redirect.from(java.io.File("/dev/null")))
            .redirectOutput(if (log == null) ProcessBuilder.Redirect.DISCARD else ProcessBuilder.Redirect.appendTo(log.toFile()))
            .redirectErrorStream(true).start()
    }) : UpdatePlatform {
    override val kind = PlatformKind.Desktop
    override suspend fun unavailable(release: ClientRelease): String? = withContext(Dispatchers.IO) {
        when {
            desktopInstallBase(launcher) == null -> "This copy was not installed with install.sh; update it the same way you installed it."
            !hasTool("minisign") -> "Install minisign on this computer before updating Porchlight."
            !hasTool("python3") -> "Install python3 on this computer before updating Porchlight."
            else -> null
        }
    }
    override suspend fun install(release: ClientRelease, source: UpdateSource, report: (UpdateState) -> Unit): UpdateState = withContext(Dispatchers.IO) {
        val base = desktopInstallBase(launcher) ?: throw UpdateFailure("The installed launcher is unavailable.")
        require(releaseVersionCode(release.version) != null)
        val files = release.desktop.files
        val required = setOf("install.sh", "release.json", "manifest.json", "manifest.json.minisig", "porchlight.svg", release.desktop.archive)
        require(files.map { it.path }.containsAll(required))
        require(files.all { it.path in required || it.path.startsWith("lib/") })
        val dir = guardedUpdateDirectory(guardedUpdateDirectory(root).resolve(release.version))
        report(UpdateState.Downloading(0, files.sumOf { it.propertySize }))
        val payload = downloadReleaseFiles(source, files, dir, report)
        val log = updatePath(dir, "installer.log")
        if (!Files.exists(log, LinkOption.NOFOLLOW_LINKS)) Files.createFile(log, updateFileMode)
        require(Files.isRegularFile(log, LinkOption.NOFOLLOW_LINKS))
        Files.setPosixFilePermissions(log, java.nio.file.attribute.PosixFilePermissions.fromString("rw-------"))
        report(UpdateState.Installing)
        // The installer owns its atomic switch: once launched, wait for its outcome even on UI cancellation.
        val exit = withContext(NonCancellable) { start(desktopInstallerArgv(base, payload), log).waitFor() }
        if (exit != 0) throw UpdateFailure("The installer could not finish. See $log")
        try { removeUpdateTree(dir) } catch (_: java.io.IOException) { /* Installed successfully; leftover staging can be removed later. */ }
        UpdateState.ReadyToRestart
    }
    override suspend fun restart() = withContext(Dispatchers.IO) {
        // current now points to the new image, so do not require it to equal the old running binary.
        val path = Path.of(checkNotNull(launcher)).normalize()
        val image = path.parent.parent.parent
        val base = if (image.fileName.toString() == "current") image.parent else image.parent.parent
        check(Files.isExecutable(base.resolve("current/app/bin/porchlight")))
        start(desktopRestartArgv(base), null)
        quit()
    }
}
