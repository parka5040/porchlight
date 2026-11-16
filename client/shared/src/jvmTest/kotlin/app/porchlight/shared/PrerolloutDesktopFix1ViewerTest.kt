package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.screens.collaboration.prepareDesktopAttachmentViewer
import app.porchlight.shared.screens.collaboration.desktopAttachmentViewerCommand
import kotlinx.coroutines.runBlocking
import java.nio.file.*
import kotlin.test.*

class PrerolloutDesktopFix1ViewerTest {
    private fun root() = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "viewer-handoff-")
    @Test fun serverPlainTextAlwaysHasPrivateTxtAliasEvenWithHtmlPrefixes() = runBlocking {
        val root = root()
        try {
            for (prefix in listOf("<HEAD", "<html")) {
                val bytes = "$prefix>plain text, not browser content".toByteArray()
                val store = FileAttachmentStore(root.toFile())
                store.write("original", bytes)
                var opened: Path? = null
                val viewer = prepareDesktopAttachmentViewer(root, "attachment", "text/plain", store.content("original")!!) { path, mime -> assertEquals("text/plain", mime); opened = Path.of(path) }
                viewer.launch()
                val alias = assertNotNull(opened)
                assertTrue(alias.fileName.toString().endsWith(".txt"), "Server text/plain must not be sniffed from an extensionless original")
                assertNotEquals(root.resolve("original"), alias)
                assertContentEquals(bytes, Files.readAllBytes(alias))
                assertTrue(Files.getPosixFilePermissions(alias).all { it.name.startsWith("OWNER_") })
                viewer.discard()
            }
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun htmlSniffingCannotOverrideServerPlainTextHandler() {
        val command = desktopAttachmentViewerCommand("/srv/data/attachment.txt", "text/plain", { kind, value ->
            if (kind == "filetype") "text/html" else { assertEquals("text/plain", value); "text-editor.desktop" }
        }, { Path.of("/srv/data/applications", it) })
        assertEquals(listOf("gio", "launch", "/srv/data/applications/text-editor.desktop", "/srv/data/attachment.txt"), command)
    }
    @Test fun matchingLocalSniffStillCannotDelegateMimeSelectionToDesktop() {
        val command = desktopAttachmentViewerCommand("/srv/data/attachment.txt", "text/plain", { kind, _ ->
            if (kind == "filetype") "text/plain" else "text-editor.desktop"
        }, { Path.of("/srv/data/applications", it) })
        assertEquals(listOf("gio", "launch", "/srv/data/applications/text-editor.desktop", "/srv/data/attachment.txt"), command,
            "A desktop opener may sniff differently; always choose the authoritative MIME handler")
    }
    @Test fun missingOrUnsafeMimeHandlerFailsClosed() {
        for (desktop in listOf("", "../browser.desktop", "browser.desktop\nother.desktop")) {
            assertFailsWith<IllegalArgumentException> {
                desktopAttachmentViewerCommand("/srv/data/attachment.txt", "text/plain", { _, _ -> desktop }, { error("Unsafe handler lookup") })
            }
        }
        assertFailsWith<IllegalArgumentException> {
            desktopAttachmentViewerCommand("/srv/data/attachment.txt", "text/plain", { _, _ -> "text-editor.desktop" }, { null })
        }
    }
    @Test fun unsafeMimeNeverReachesSystemViewer() = runBlocking {
        val root = root()
        try {
            val content = object : AttachmentContent {
                override suspend fun bytes() = "<html>".toByteArray()
                override suspend fun chunks(consume: suspend (ByteArray) -> Unit) { consume(bytes()) }
            }
            assertFailsWith<IllegalArgumentException> {
                prepareDesktopAttachmentViewer(root, "attachment", "text/html", content) { _, _ -> error("Unsafe MIME handed off") }
            }
            Unit
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun delayedViewerCanReadAliasAfterOriginalLruEvictionButRevocationStillRemovesIt() = runBlocking {
        val root = root()
        try {
            val store = FileAttachmentStore(root.toFile(), byteBudget = 8)
            val bytes = "text".toByteArray()
            store.write("original", bytes)
            var handedOff: Path? = null
            val viewer = prepareDesktopAttachmentViewer(root, "attachment", "text/plain", store.content("original")!!) { path, _ -> handedOff = Path.of(path) }
            viewer.launch() // xdg-open has returned; the eventual viewer has not opened its pathname yet.
            repeat(4) { store.write("other$it", ByteArray(4)) }
            store.cleanup(kotlin.time.Instant.fromEpochSeconds(0))
            assertNull(store.content("original"))
            assertContentEquals(bytes, Files.readAllBytes(assertNotNull(handedOff)), "LRU must not revoke a handed-off viewer pathname")
            store.removeView("attachment")
            assertFalse(Files.exists(handedOff))
        } finally { root.toFile().deleteRecursively() }
    }
}
