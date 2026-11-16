package app.porchlight.shared

import app.porchlight.shared.screens.collaboration.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

class PrerolloutClientUiFix1ViewerTest {
    @Test fun expandsMetadataAsLiteralArguments() {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of("build/test-fixtures/desktop-viewer")), "metadata-")
        try {
            val entry = root.resolve("viewer.desktop")
            Files.writeString(entry, "[Desktop Entry]\nType=Application\nName=Fixture ; viewer\nIcon=fixture icon\nExec=viewer %c %k %i %% %f\n")
            assertEquals(listOf("viewer", "Fixture ; viewer", entry.toString(), "--icon", "fixture icon", "%", "/srv/data/private alias.txt"),
                desktopAttachmentViewerCommand("/srv/data/private alias.txt", "text/plain", { _, _ -> "viewer.desktop" }, { entry }, false))
            Files.writeString(entry, "[Desktop Entry]\nType=Application\nName=Fixture viewer\nExec=viewer %i %c %f\n")
            assertEquals(listOf("viewer", "Fixture viewer", "/srv/data/private alias.txt"),
                desktopAttachmentViewerCommand("/srv/data/private alias.txt", "text/plain", { _, _ -> "viewer.desktop" }, { entry }, false))
        } finally { root.toFile().deleteRecursively() }
    }

    @Test fun nestedIdsRespectRootPrecedenceAndHiddenTombstones() {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of("build/test-fixtures/desktop-viewer")), "nested-").toAbsolutePath()
        try {
            val home = Files.createDirectories(root.resolve("home/applications/vendor"))
            val system = Files.createDirectories(root.resolve("system/applications"))
            val nested = home.resolve("viewer.desktop")
            val flat = system.resolve("vendor-viewer.desktop")
            Files.writeString(nested, "[Desktop Entry]\nType=Application\n")
            Files.writeString(flat, "[Desktop Entry]\nType=Application\n")
            assertEquals(nested, desktopAttachmentHandler("vendor-viewer.desktop", home.parent.parent.toString(), system.parent.toString()))
            Files.writeString(nested, "[Desktop Entry]\nHidden=true\n")
            assertNull(desktopAttachmentHandler("vendor-viewer.desktop", home.parent.parent.toString(), system.parent.toString()), "Hidden nested entry masks system handler")
            Files.writeString(nested, "[Desktop Entry]\nType=Application\n")
            Files.delete(flat)
            assertEquals(nested, desktopAttachmentHandler("vendor-viewer.desktop", home.parent.parent.toString(), system.parent.toString()))
        } finally { root.toFile().deleteRecursively() }
    }
}
