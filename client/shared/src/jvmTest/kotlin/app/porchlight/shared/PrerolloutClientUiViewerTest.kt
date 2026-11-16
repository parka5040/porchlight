package app.porchlight.shared

import app.porchlight.shared.screens.collaboration.desktopAttachmentViewerCommand
import java.nio.file.Files
import java.nio.file.Path
import app.porchlight.shared.screens.collaboration.desktopAttachmentHandler
import kotlin.test.*

class PrerolloutClientUiViewerTest {
    private fun command(exec: String, extra: String = "", gio: Boolean = false): List<String> {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of("build/test-fixtures/desktop-viewer")), "entry-")
        try {
            val entry = root.resolve("viewer.desktop")
            Files.writeString(entry, "[Desktop Entry]\nType=Application\nName=Fixture viewer\nExec=$exec\n$extra\n[Desktop Action Other]\nExec=wrong\n")
            return desktopAttachmentViewerCommand("/srv/data/private alias.txt", "text/plain", { kind, mime ->
                assertEquals("default", kind); assertEquals("text/plain", mime); "viewer.desktop"
            }, { entry }, gioAvailable = gio)
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun missingGioLaunchesServerMimeDesktopEntryDirectly() {
        assertEquals(listOf("fixture-viewer", "--read-only", "/srv/data/private alias.txt"), command("fixture-viewer --read-only %f"))
    }
    @Test fun quotingCodesAndMetacharactersNeverUseAShell() {
        for (code in listOf("%f", "%F", "%u", "%U")) {
            assertEquals(listOf("/srv/data/fixture viewer", "literal;$(not-a-shell)", "/srv/data/private alias.txt", "%"),
                command("\"/srv/data/fixture viewer\" \"literal;$(not-a-shell)\" $code %i %%"))
        }
        assertEquals(listOf("viewer", "/srv/data/private alias.txt"), command("viewer"))
        val quote = "\\".repeat(2) + "\""
        val slash = "\\".repeat(4)
        assertEquals(listOf("viewer", "a\"b", "c\\d", "/srv/data/private alias.txt"), command("viewer \"a${quote}b\" \"c${slash}d\" %f"))
    }
    @Test fun unsafeUnsupportedOrTerminalEntriesFailClosed() {
        for (exec in listOf("viewer %x", "viewer --file=%f", "viewer \"%f\"", "viewer %f %U", "viewer \"unterminated", "viewer trailing\\", "")) {
            assertFails("Must reject $exec") { command(exec) }
        }
        assertFails { command("viewer %f", "Terminal=true") }
        assertFails { command("viewer %f", "Hidden=true") }
        assertFails { command("viewer %f", "Type=Link") }
    }
    @Test fun xdgDataHomeOverridesDataDirsWithoutDesktopSpecificTools() {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of("build/test-fixtures/desktop-viewer")), "xdg-").toAbsolutePath()
        try {
            val home = Files.createDirectories(root.resolve("home/applications"))
            val system = Files.createDirectories(root.resolve("system/applications"))
            val second = Files.createDirectories(root.resolve("second/applications"))
            for (directory in listOf(home, system, second)) Files.writeString(directory.resolve("viewer.desktop"), "[Desktop Entry]\n")
            val dirs = "${system.parent}:${second.parent}"
            assertEquals(home.resolve("viewer.desktop"), desktopAttachmentHandler("viewer.desktop", home.parent.toString(), dirs))
            Files.delete(home.resolve("viewer.desktop"))
            assertEquals(system.resolve("viewer.desktop"), desktopAttachmentHandler("viewer.desktop", home.parent.toString(), dirs))
            assertFails { desktopAttachmentHandler("../viewer.desktop", home.parent.toString(), dirs) }
            assertNull(desktopAttachmentHandler("missing.desktop", "relative", "relative"))
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun gioRemainsPreferred() {
        assertEquals("gio", command("viewer %x", gio = true).first())
    }
}
