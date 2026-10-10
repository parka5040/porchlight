package app.porchlight.shared

import app.porchlight.shared.live.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

class DesktopLiveTest {
    @Test fun secondInstanceRequestsFocusWithoutTakingTheLock() {
        val dir = Files.createTempDirectory(Path.of(System.getProperty("porchlight.test.dir")), "singleton-")
        try {
            val first = DesktopInstance.acquire(dir)!!
            try { assertNull(DesktopInstance.acquire(dir)); assertTrue(first.focusRequested()); assertFalse(first.focusRequested()) }
            finally { first.close() }
            assertNotNull(DesktopInstance.acquire(dir)).close()
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun autostartUsesQuotedAbsoluteLauncherAndNoSystemd() {
        val text = desktopAutostart("/opt/Porch light/bin/Porchlight")
        assertTrue(text.contains("Exec=\"/opt/Porch light/bin/Porchlight\" --tray"))
        assertTrue(text.contains("Type=Application")); assertFalse(text.contains("systemd"))
        assertEquals("spawn-at-startup \"/opt/Porch light/bin/Porchlight\" \"--tray\"", niriAutostart("/opt/Porch light/bin/Porchlight"))
        assertFailsWith<IllegalArgumentException> { desktopAutostart("relative/path") }
        assertFailsWith<IllegalArgumentException> { desktopAutostart("/opt/bad\nlauncher") }
    }
}
