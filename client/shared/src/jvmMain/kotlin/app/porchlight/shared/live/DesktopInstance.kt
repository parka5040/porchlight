package app.porchlight.shared.live

import java.nio.channels.FileChannel
import java.nio.channels.FileLock
import java.nio.channels.OverlappingFileLockException
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions

class DesktopInstance private constructor(private val directory: Path, private val channel: FileChannel, private val lock: FileLock) : AutoCloseable {
    private val focusDirectory = directory.resolve("focus").also { Files.createDirectories(it) }
    private val watcher = directory.fileSystem.newWatchService().also {
        focusDirectory.register(it, StandardWatchEventKinds.ENTRY_CREATE, StandardWatchEventKinds.ENTRY_MODIFY)
    }
    suspend fun awaitFocusRequest(): Boolean = kotlinx.coroutines.runInterruptible(kotlinx.coroutines.Dispatchers.IO) {
        if (focusRequested()) return@runInterruptible true
        try { while (true) {
            val key = watcher.take()
            val requested = key.pollEvents().any { it.kind() == StandardWatchEventKinds.OVERFLOW || it.context().toString() == "focus-request" }
            if (!key.reset()) return@runInterruptible false
            if (requested && focusRequested()) return@runInterruptible true
        }
        } catch (_: ClosedWatchServiceException) { return@runInterruptible false }
        @Suppress("UNREACHABLE_CODE") false
    }
    fun focusRequested(): Boolean = Files.deleteIfExists(directory.resolve("focus/focus-request"))
    override fun close() { watcher.close(); lock.release(); channel.close() }
    companion object {
        fun acquire(directory: Path, afterUpdate: Boolean = false): DesktopInstance? {
            val deadline = System.nanoTime() + 10_000_000_000L
            do {
                acquireOnce(directory, requestFocus = false)?.let { return it }
                if (!afterUpdate || System.nanoTime() >= deadline) break
                Thread.sleep(100)
            } while (true)
            return acquireOnce(directory, requestFocus = true)
        }
        private fun acquireOnce(directory: Path, requestFocus: Boolean): DesktopInstance? {
            Files.createDirectories(directory)
            Files.createDirectories(directory.resolve("focus"))
            Files.setPosixFilePermissions(directory, PosixFilePermissions.fromString("rwx------"))
            val channel = FileChannel.open(directory.resolve("instance.lock"), StandardOpenOption.CREATE, StandardOpenOption.WRITE)
            val lock = try { channel.tryLock() } catch (_: OverlappingFileLockException) { null }
            if (lock == null) { channel.close(); if (requestFocus) Files.writeString(directory.resolve("focus/focus-request"), "focus"); return null }
            Files.deleteIfExists(directory.resolve("focus/focus-request"))
            return DesktopInstance(directory, channel, lock)
        }
    }
}
private fun launcher(value: String, shellEscapes: Boolean = true): String {
    require(Path.of(value).isAbsolute && value.none { it == '\n' || it == '\r' || it == '\u0000' })
    val quoted = value.replace("\\", "\\\\").replace("\"", "\\\"")
    return if (shellEscapes) quoted.replace("`", "\\`").replace("$", "\\$") else quoted
}
fun desktopAutostart(path: String): String = "[Desktop Entry]\nType=Application\nName=Porchlight\nExec=\"${launcher(path).replace("%", "%%")}\" --tray\nTerminal=false\n"
fun niriAutostart(path: String): String = "spawn-at-startup \"${launcher(path, shellEscapes = false)}\" \"--tray\""
enum class DesktopAutostartResult { Enabled, Disabled, NiriManaged }
fun writeDesktopAutostart(configDirectory: Path, path: String, enabled: Boolean): DesktopAutostartResult {
    val file = configDirectory.resolve("autostart/porchlight.desktop")
    if (!enabled) { Files.deleteIfExists(file); return DesktopAutostartResult.Disabled }
    val niri = configDirectory.resolve("niri/config.kdl")
    val niriEnabled = Files.exists(niri) && Files.readAllLines(niri).any {
        it.trimStart().startsWith("spawn-at-startup") && it.contains("/bin/porchlight\"", ignoreCase = true)
    }
    if (niriEnabled) {
        Files.deleteIfExists(file)
        return DesktopAutostartResult.NiriManaged
    }
    val text = desktopAutostart(path)
    Files.createDirectories(file.parent); Files.writeString(file, text)
    return DesktopAutostartResult.Enabled
}

/** jpackage injects app-path; map its renamed native binary back to the pre-JVM wrapper, never Java. */
fun resolveDesktopLauncher(packagedPath: String?, command: String?, stablePath: Path? = null): String? {
    val wrapper = listOfNotNull(packagedPath, command).firstNotNullOfOrNull { value ->
        runCatching {
            val path = Path.of(value)
            if (!path.isAbsolute) null else when {
                path.fileName.toString().equals("porchlight", ignoreCase = true) -> path
                path.fileName.toString() == "porchlight-app" -> path.resolveSibling("porchlight")
                    .takeIf { Files.isRegularFile(it) && Files.isExecutable(it) }
                else -> null
            }
        }.getOrNull()
    } ?: return null
    // Stable current tracks upgrades only when it points to this image's wrapper, not another installation.
    return if (stablePath != null && runCatching { Files.isSameFile(stablePath, wrapper) }.getOrDefault(false))
        stablePath.toString() else wrapper.toString()
}

@Suppress("UNUSED_PARAMETER")
fun desktopCloseHides(trayAvailable: Boolean, notice: String?): Boolean = trayAvailable
