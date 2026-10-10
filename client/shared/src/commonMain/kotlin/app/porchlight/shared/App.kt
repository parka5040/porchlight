package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.Alignment
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.ui.ErrorState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.collect

@Composable
fun PorchlightApp(services: AppServices, gate: @Composable (content: @Composable () -> Unit) -> Unit = { it() },
    accountContent: (@Composable () -> Unit)? = null) {
    var settings by remember(services) { mutableStateOf<DeviceSettings?>(null) }
    val portalDark by services.platform.systemDark.collectAsState()
    val systemDark = if (services.platform.kind == PlatformKind.Android) isSystemInDarkTheme() else portalDark
    var saveableRoute by rememberSaveable { mutableStateOf<String?>(null) }
    // The gate removes UI on relock, not its saved navigation/back stack.
    val shellState = rememberSaveableStateHolder()
    var startRoute by remember { mutableStateOf<AppRoute?>(null) }
    var loadError by remember { mutableStateOf(false) }
    var retry by remember { mutableIntStateOf(0) }
    LaunchedEffect(services, retry) {
        try {
            services.settings.settings.collect { stored ->
                if (startRoute == null) {
                    val restore = if (saveableRoute != null) stored.copy(currentRoute = RouteCodec.decode(saveableRoute), startScreen = StartScreen.LastScreen) else stored
                    startRoute = restoreRoute(restore, services::routeAvailable)
                    saveableRoute = RouteCodec.encode(startRoute!!)
                }
                settings = stored
                loadError = false
            }
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { reportDiagnostic("Loading device settings", e); loadError = true }
    }
    PorchlightTheme(settings?.themeMode ?: ThemeMode.System, systemDark) {
        Surface(Modifier.fillMaxSize().testTag("app-background"), color = MaterialTheme.colorScheme.background) {
            // Nothing restored is composed outside this gate (PIN/trust wrapper supplied by C1b/C2).
            gate {
                val route = startRoute
                when {
                    route != null && settings != null -> shellState.SaveableStateProvider("shell") {
                        CompositionLocalProvider(app.porchlight.shared.screens.LocalScreenServices provides services) {
                            ShellNavigation(route, settings!!, services.settings,
                                savedRoute = { saveableRoute = RouteCodec.encode(it) }, loadErrorRetry = if (loadError) ({ retry++ }) else null, accountContent = accountContent, services = services)
                        }
                    }
                    loadError -> Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
                        ErrorState("Device settings could not be loaded.", { retry++ })
                    }
                    else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("Opening Porchlight…") }
                }
            }
        }
    }
}

