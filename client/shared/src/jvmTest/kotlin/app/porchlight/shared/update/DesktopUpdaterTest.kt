package app.porchlight.shared.update

import app.porchlight.shared.api.models.ReleaseFile
import app.porchlight.shared.backups.DownloadHead
import app.porchlight.shared.live.DesktopInstance
import kotlinx.coroutines.*
import java.nio.file.*
import kotlin.test.*

class DesktopUpdaterTest {
    private fun install(root: Path, script: String = "exit 0\n"): Path {
        val image = Files.createDirectories(root.resolve("releases/0.1.6/app/bin"))
        Files.writeString(image.resolve("porchlight"), "#!/bin/sh\nexit 0\n").toFile().setExecutable(true)
        Files.writeString(root.resolve("releases/0.1.6/install.sh"), script)
        Files.createSymbolicLink(root.resolve("current"), Path.of("releases/0.1.6"))
        return root.resolve("current/app/bin/porchlight")
    }
    @Test fun eligibilityRequiresThisInstalledLauncherAndBothTools() = runBlocking {
        val root = updateTestRoot()
        try {
            assertNull(desktopInstallBase("relative/app/bin/porchlight"))
            assertNull(desktopInstallBase(root.resolve("app/bin/porchlight").toString()))
            val launcher = install(root)
            assertEquals(root, desktopInstallBase(launcher.toString()))
            assertEquals(root, desktopInstallBase(launcher.toRealPath().toString()))
            assertNull(DesktopUpdater(launcher.toString(), root.resolve("updates"), { true }).unavailable(updateRelease()))
            assertTrue(DesktopUpdater(launcher.toString(), root.resolve("updates"), { it != "minisign" }).unavailable(updateRelease())!!.contains("minisign"))
            assertTrue(DesktopUpdater(launcher.toString(), root.resolve("updates"), { it != "python3" }).unavailable(updateRelease())!!.contains("python3"))
            assertTrue(DesktopUpdater(null, root.resolve("updates"), { true }).unavailable(updateRelease())!!.contains("install.sh"))
            assertEquals(Path.of("/state/porchlight/updates"), desktopUpdateDirectory("/state", "/fixture"))
            assertEquals(Path.of("/fixture/.local/state/porchlight/updates"), desktopUpdateDirectory("relative", "/fixture"))
        } finally { removeUpdateTree(root) }
    }
    @Test fun trustedInstallerArgvAndFailureLogNeverExecuteDownloadedInstaller() = runBlocking {
        for (exit in listOf(0, 7)) {
            val root = updateTestRoot()
            try {
                val argv = root.resolve("argv")
                val script = "printf '%s\\n' \"\$@\" > '${argv}'\nprintf 'private installer output\\n'\nexit $exit\n"
                val launcher = install(root, script)
                val names = listOf("install.sh", "release.json", "manifest.json", "manifest.json.minisig", "porchlight.svg", "lib/operations.py", updateRelease().desktop.archive)
                val bytes = "fixture".toByteArray()
                val files = names.map { ReleaseFile(it, bytes.size.toLong(), sha(bytes)) }
                val release = updateRelease(files = files)
                val source = object : TestUpdateSource(release) {
                    override suspend fun download(file: ReleaseFile, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                        begin(DownloadHead(200, "\"${file.sha256}\"", file.propertySize, null)); chunk(bytes)
                    }
                }
                val updater = DesktopUpdater(launcher.toString(), root.resolve("updates"), { true })
                val states = mutableListOf<UpdateState>()
                if (exit == 0) {
                    assertEquals(UpdateState.ReadyToRestart, updater.install(release, source, states::add))
                    assertFalse(Files.exists(root.resolve("updates/0.1.7")))
                } else {
                    val failure = assertFailsWith<UpdateFailure> { updater.install(release, source, states::add) }
                    assertTrue(failure.reason.contains("installer.log")); assertFalse(failure.reason.contains("private installer output"))
                    assertEquals("private installer output\n", Files.readString(root.resolve("updates/0.1.7/installer.log")))
                }
                assertEquals(listOf("desktop", "--source", root.resolve("updates/0.1.7/source").toString()), Files.readAllLines(argv))
                assertTrue(states.contains(UpdateState.Installing)); assertTrue(states.contains(UpdateState.Verifying))
                assertEquals(listOf(launcher.toString(), "--after-update"), desktopRestartArgv(root))
            } finally { removeUpdateTree(root) }
        }
    }
    @Test fun afterUpdateWaitsForOldLockInsteadOfFocusingIt() = runBlocking {
        val root = updateTestRoot()
        try {
            val old = assertNotNull(DesktopInstance.acquire(root))
            val replacement = async(Dispatchers.IO) { DesktopInstance.acquire(root, afterUpdate = true) }
            delay(250)
            assertFalse(replacement.isCompleted)
            assertFalse(old.focusRequested())
            old.close()
            assertNotNull(withTimeout(3000) { replacement.await() }).close()
        } finally { removeUpdateTree(root) }
    }
    @Test fun normalSecondInstanceStillRequestsFocus() {
        val root = updateTestRoot()
        try {
            assertNotNull(DesktopInstance.acquire(root)).use { first ->
                assertNull(DesktopInstance.acquire(root)); assertTrue(first.focusRequested())
            }
        } finally { removeUpdateTree(root) }
    }
}
