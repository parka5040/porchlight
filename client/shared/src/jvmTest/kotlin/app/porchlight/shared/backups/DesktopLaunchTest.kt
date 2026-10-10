package app.porchlight.shared.backups

import kotlin.test.*

class DesktopLaunchTest {
    @Test fun installerLaunchAndBenignFlagsStartTheApp() {
        assertFalse(desktopLaunch(emptyArray()).minimized)
        // Both XDG Exec and niri spawn-at-startup pass this exact argument.
        assertTrue(desktopLaunch(arrayOf("--minimized")).minimized)
        assertTrue(desktopLaunch(arrayOf("--tray")).minimized)
        assertTrue(desktopLaunch(arrayOf("--tray", "--minimized")).minimized)
        assertTrue(desktopLaunch(arrayOf("--benign-future-flag", "--minimized")).minimized)
        assertNull(desktopLaunch(arrayOf("--benign-future-flag")).smokeDevice)
    }
    @Test fun smokePrivatePathsAreBoundToTheProjectRuntimeRoot() {
        val root = java.nio.file.Path.of("/project/var")
        requireSmokeRuntimePath(root.resolve("gate/home"), root)
        assertFailsWith<IllegalArgumentException> { requireSmokeRuntimePath(java.nio.file.Path.of("/var/tmp/not-private"), root) }
        assertFailsWith<IllegalArgumentException> { requireSmokeRuntimePath(java.nio.file.Path.of("/project/var/../elsewhere"), root) }
    }
    @Test fun smokeIsInterceptedOnlyAsTheFirstArgumentAndValidated() {
        val smoke = desktopLaunch(arrayOf("--backup-pull-smoke", "var/devices/desktop", "https://localhost:19443"))
        assertEquals("var/devices/desktop", smoke.smokeDevice)
        assertEquals("https://localhost:19443", smoke.smokeOrigin)
        assertFailsWith<IllegalArgumentException> { desktopLaunch(arrayOf("--backup-pull-smoke")) }
        assertNull(desktopLaunch(arrayOf("--minimized", "--backup-pull-smoke")).smokeDevice)
    }
}
