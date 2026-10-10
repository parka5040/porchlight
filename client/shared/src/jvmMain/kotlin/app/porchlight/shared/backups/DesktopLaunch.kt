package app.porchlight.shared.backups

/** Only the explicit controller command is intercepted; installer/future benign flags remain app launches. */
data class DesktopLaunch(val minimized: Boolean = false, val smokeDevice: String? = null, val smokeOrigin: String? = null)
fun desktopLaunch(args: Array<String>): DesktopLaunch {
    if (args.firstOrNull() == "--backup-pull-smoke") {
        require(args.size == 3) { "Usage: --backup-pull-smoke DEVICE_DIR ORIGIN" }
        return DesktopLaunch(smokeDevice = args[1], smokeOrigin = args[2])
    }
    return DesktopLaunch(minimized = "--minimized" in args || "--tray" in args)
}
