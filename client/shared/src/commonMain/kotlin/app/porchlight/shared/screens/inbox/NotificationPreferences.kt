package app.porchlight.shared.screens.inbox

import app.porchlight.shared.theme.PorchlightTokens

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.AppServices
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.launch

// DESIGN-PENDING: preference detail and version recovery.
@Composable fun NotificationPreferences(services: AppServices?) {
    val data = services?.data
    if (data == null) { DataStatus(null); return }
    val device by services.settings.settings.collectAsState(initial = app.porchlight.shared.settings.DeviceSettings())
    val view by data.cache.view.collectAsState()
    val current = view.notificationPrefs
    val scope = applicationWriteScope()
    val writer = screenWrites("notification-preferences")
    if (current == null) { DataStatus(data); return }
    // A version change reseeds only after an authoritative response; the pending writer retains its original request.
    var draft by remember(current.version) { mutableStateOf(current.prefs) }
    var start by remember(current.version) { mutableStateOf(current.prefs.quietStart.orEmpty()) }
    var end by remember(current.version) { mutableStateOf(current.prefs.quietEnd.orEmpty()) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        val connectionLabel = if (services.platform.kind == app.porchlight.shared.PlatformKind.Desktop) "Start at login" else "Stay connected on this device"
        Row(Modifier.fillMaxWidth().heightIn(min = PorchlightTokens.touchTarget.dp)) {
            Text(connectionLabel, modifier = Modifier.weight(1f))
            Switch(device.stayConnected, { value -> scope.launch { services.settings.setStayConnected(value) } }, modifier = Modifier.semantics { contentDescription = connectionLabel })
        }
        services.desktopLoginConflict?.let { state ->
            val conflict by state.collectAsState()
            conflict?.let { text -> Text(text); ShellButton("Dismiss", onClick = { services.dismissDesktopLoginConflict?.invoke() }) }
        }
        services.desktopLoginHelp?.let { androidx.compose.foundation.text.selection.SelectionContainer { Text(it) } }
        services.runtimeNotice?.let { notice -> val text by notice.collectAsState(); text?.let { Text(it) } }
        services.recoverNotifications?.let { ShellButton("Android notification settings", onClick = it) }
        services.recoverExactAlarms?.let { ShellButton("Android Alarms & reminders", onClick = it) }
        Text("Notification preferences", style = MaterialTheme.typography.headlineSmall)
        Text("These delivery categories sync across devices. Comments and conflicts use Household activity.")
        listOf("Reminders" to draft.reminders, "Assignments" to draft.assignments,
            "Dependents' appointments" to draft.dependents, "Household activity" to draft.activity,
            "Digest" to draft.digest, "Connection" to draft.connection).forEach { (label, enabled) ->
            Row(Modifier.fillMaxWidth().heightIn(min = PorchlightTokens.touchTarget.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(label, modifier = Modifier.weight(1f))
                Switch(enabled, { value -> draft = when (label) {
                    "Reminders" -> draft.copy(reminders = value); "Assignments" -> draft.copy(assignments = value)
                    "Dependents' appointments" -> draft.copy(dependents = value); "Household activity" -> draft.copy(activity = value)
                    "Digest" -> draft.copy(digest = value); else -> draft.copy(connection = value)
                } }, modifier = Modifier.semantics { contentDescription = label })
            }
        }
        Text("Quiet hours use your member time zone. Leave both fields empty to disable.")
        OutlinedTextField(start, { start = it }, label = { Text("Quiet start (HH:MM:SS)") }, modifier = Modifier.fillMaxWidth())
        OutlinedTextField(end, { end = it }, label = { Text("Quiet end (HH:MM:SS)") }, modifier = Modifier.fillMaxWidth())
        val feedback by writer.state.collectAsState()
        WriteStatus(writer, onDiscard = { scope.refreshSafely(data) })
        if (feedback.conflict) Text("Preferences changed on another device. Discard and refetch before editing again.")
        ShellButton("Save notification preferences", onClick = {
            val request = draft.copy(quietStart = start.takeIf { it.isNotBlank() }, quietEnd = end.takeIf { it.isNotBlank() })
            val expected = current.version
            scope.launch { writer.submit { data.notifications.putPrefs(expected, request, it).feedback() } }
        }, enabled = !feedback.conflict)
    }
}
