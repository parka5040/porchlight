package app.porchlight.shared

import app.porchlight.shared.live.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

class DesktopLauncherPathTest {
    @Test fun installedWrapperKeepsStableCurrentAcrossUpgrades() {
        val directory = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "stable-launcher-")
        try {
            val versions = listOf("0.1.1", "0.1.2").map { version ->
                val image = Files.createDirectories(directory.resolve("releases/$version/app/bin"))
                Files.writeString(image.resolve("porchlight"), "#!/bin/sh\n").toFile().setExecutable(true)
                Files.writeString(image.resolve("porchlight-app"), "native")
                image
            }
            val current = directory.resolve("current")
            val stable = current.resolve("app/bin/porchlight")
            Files.createSymbolicLink(current, versions[0].parent.parent)
            assertEquals(stable.toString(), resolveDesktopLauncher(versions[0].resolve("porchlight-app").toString(), null, stable))
            Files.delete(current)
            Files.createSymbolicLink(current, versions[1].parent.parent)
            assertEquals(stable.toString(), resolveDesktopLauncher(versions[1].resolve("porchlight-app").toString(), null, stable))
            // A still-running old version must not claim the new image is the same file.
            assertEquals(versions[0].resolve("porchlight").toString(), resolveDesktopLauncher(versions[0].resolve("porchlight-app").toString(), null, stable))
            assertEquals(stable.toString(), resolveDesktopLauncher(versions[1].resolve("porchlight").toString(), null, stable))
        } finally { directory.toFile().deleteRecursively() }
    }

    @Test fun renamedNativeLauncherResolvesToWrapperForEveryWrittenPath() {
        val directory = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "wrapped-launcher-")
        try {
            val bin = Files.createDirectories(directory.resolve("app/bin"))
            val wrapper = Files.writeString(bin.resolve("porchlight"), "#!/bin/sh\n")
            wrapper.toFile().setExecutable(true)
            val native = Files.writeString(bin.resolve("porchlight-app"), "native")
            val resolved = resolveDesktopLauncher(native.toString(), "/usr/bin/java")
            assertEquals(wrapper.toString(), resolved)
            assertEquals(wrapper.toString(), resolveDesktopLauncher(null, native.toString()))
            assertEquals(niriAutostart(wrapper.toString()), niriAutostart(resolved!!))
            writeDesktopAutostart(directory.resolve("config"), resolved, true)
            val entry = Files.readString(directory.resolve("config/autostart/porchlight.desktop"))
            assertTrue(entry.contains("/bin/porchlight\" --tray"))
            assertFalse(entry.contains("porchlight-app"))
            Files.delete(wrapper)
            assertNull(resolveDesktopLauncher(native.toString(), "/usr/bin/java"))
        } finally { directory.toFile().deleteRecursively() }
    }
}
