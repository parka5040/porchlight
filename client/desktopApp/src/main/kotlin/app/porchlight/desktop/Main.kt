package app.porchlight.desktop

import androidx.compose.runtime.*
import kotlinx.coroutines.*
import java.nio.file.Path
import java.awt.EventQueue
import app.porchlight.shared.live.DesktopInstance
import app.porchlight.shared.backups.desktopLaunch
import app.porchlight.shared.backups.desktopTrayActions
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import app.porchlight.shared.TrustedPorchlightApp
import app.porchlight.shared.BuildInfo
import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.settings.DesktopAppServices
import app.porchlight.shared.ui.PorchlightWindowIcon

fun main(args: Array<String>) {
    val launch = try { desktopLaunch(args) } catch (_: IllegalArgumentException) { kotlin.system.exitProcess(2) }
    if (launch.smokeDevice != null) {
        kotlin.system.exitProcess(app.porchlight.shared.backups.backupPullSmoke(Path.of(launch.smokeDevice), launch.smokeOrigin!!))
    }
    val directory = Path.of(System.getenv("XDG_DATA_HOME") ?: (System.getProperty("user.home") + "/.local/share"), "porchlight")
    val instance = DesktopInstance.acquire(directory, afterUpdate = "--after-update" in args) ?: return
    application {
    var visible by remember { mutableStateOf(!launch.minimized) }
    var focus by remember { mutableIntStateOf(0) }
    val services = remember { DesktopAppServices() }
    LaunchedEffect(services) {
        val actions = desktopTrayActions(services.backups.state.value,
            { EventQueue.invokeLater { visible = true; focus++ } }, { EventQueue.invokeLater { exitApplication() } })
        services.startLive(actions.open.invoke, actions.quit.invoke)
        launch { services.trayAvailable.collect { if (it == false) visible = true } }
        while (isActive) { if (instance.awaitFocusRequest()) { visible = true; focus++ } else break }
    }
    val controller = remember { services.trust.controller }
    DisposableEffect(services, controller) { onDispose { controller.close(); services.close(); instance.close() } }
    Window(visible = visible, onCloseRequest = { if (app.porchlight.shared.live.desktopCloseHides(services.trayAvailable.value == true)) visible = false else exitApplication() }, title = "Porchlight", icon = PorchlightWindowIcon()) {
        DisposableEffect(window, controller) {
            val listener = object : java.awt.event.WindowFocusListener {
                override fun windowGainedFocus(event: java.awt.event.WindowEvent) { controller.onForeground(); services.updates.onResume() }
                override fun windowLostFocus(event: java.awt.event.WindowEvent) = Unit
            }
            window.addWindowFocusListener(listener)
            onDispose { window.removeWindowFocusListener(listener) }
        }
        LaunchedEffect(focus, visible) { if (visible) { window.toFront(); window.requestFocus() } }
        TrustedPorchlightApp(services, controller)
    }
}

}
