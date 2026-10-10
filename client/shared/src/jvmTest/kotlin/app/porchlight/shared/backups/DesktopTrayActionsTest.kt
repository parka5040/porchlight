package app.porchlight.shared.backups

import kotlin.test.*
import kotlin.time.Instant

class DesktopTrayActionsTest {
    @Test fun contributesActionsWithoutCreatingATray() {
        var opened = 0
        var quit = 0
        val actions = desktopTrayActions(BackupView(), { opened++ }, { quit++ })
        assertEquals("Open Porchlight", actions.open.label)
        assertEquals("Quit", actions.quit.label)
        assertNull(actions.backupStatus)
        actions.open.invoke()
        actions.quit.invoke()
        assertEquals(1, opened)
        assertEquals(1, quit)
    }

    @Test fun backupStatusIsFencedAndDoesNotExposePrivatePathsOrErrors() {
        fun label(view: BackupView) = desktopTrayActions(view, {}, {}).backupStatus
        val privateView = BackupView(localBundle = "/private/bundle.tar.age", error = "private error")
        assertNull(label(privateView))
        assertEquals("Backups: can't reach home", label(BackupView(unreachableSince = Instant.fromEpochSeconds(1))))
        assertEquals("Backups: pulling…", label(BackupView(allowed = true, pulling = true)))
        assertEquals("Backups: needs attention", label(privateView.copy(allowed = true)))
        assertEquals("Backups: local bundle verified", label(privateView.copy(allowed = true, error = null)))
        assertEquals("Backups: no local bundle this session", label(BackupView(allowed = true)))
    }
}
