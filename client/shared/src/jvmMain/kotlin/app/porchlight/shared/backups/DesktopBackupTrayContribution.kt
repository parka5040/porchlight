package app.porchlight.shared.backups

/** Data-only contribution for C4's tray owner; this never creates an icon or sends a notification. */
data class DesktopTrayAction(val label: String, val invoke: () -> Unit)
data class DesktopBackupTrayContribution(
    val open: DesktopTrayAction,
    val quit: DesktopTrayAction,
    val backupStatus: String?,
)

fun desktopTrayActions(view: BackupView, onOpen: () -> Unit, onQuit: () -> Unit) = DesktopBackupTrayContribution(
    open = DesktopTrayAction("Open Porchlight", onOpen),
    quit = DesktopTrayAction("Quit", onQuit),
    backupStatus = when {
        view.cantReachHome -> "Backups: can't reach home"
        !view.allowed -> null
        view.pulling -> "Backups: pulling…"
        view.error != null || view.status?.lastError != null -> "Backups: needs attention"
        view.localBundle != null -> "Backups: local bundle verified"
        else -> "Backups: no local bundle this session"
    },
)

/** Status-only contribution to C4's one SNI registry; Open/Quit are its existing built-ins. */
fun app.porchlight.shared.live.DesktopTrayActions.registerBackupStatus(view: BackupView) {
    val label = desktopTrayActions(view, {}, {}).backupStatus
    if (label == null) unregister("backup-status")
    else if (entry("backup-status")?.label != label) register("backup-status", label, enabled = false) {}
}
