package app.porchlight.shared.update

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.models.ServerRelease
import app.porchlight.shared.theme.PorchlightTokens
import app.porchlight.shared.ui.*

@Composable
fun UpdateBanner(updates: UpdateController, dismissed: String?) {
    val state by updates.state.collectAsState()
    val available = state as? UpdateState.Available
    if (available != null && available.release.version != dismissed) {
        UpdateBannerContent(available.release.version, { updates.update() }, { updates.dismiss(available.release.version) })
    } else if (state !is UpdateState.Available && state !is UpdateState.Unavailable && state != UpdateState.Idle) {
        PorchlightCard(Modifier.fillMaxWidth().padding(PorchlightTokens.spaceSm.dp)) { UpdateStatus(updates) }
    }
}
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun UpdateBannerContent(version: String, update: () -> Unit, later: () -> Unit) {
    PorchlightCard(Modifier.fillMaxWidth().padding(PorchlightTokens.spaceSm.dp)) {
        Text("Porchlight $version is available", style = MaterialTheme.typography.titleMedium)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
            PrimaryAction("Update", update)
            SecondaryAction("Later", later)
        }
        DesignPending("Client update banner")
    }
}
@Composable
fun UpdateAbout(updates: UpdateController?) {
    val server = updates?.serverVersion?.collectAsState()?.value
    val status = updates?.serverRelease?.collectAsState()?.value
    LaunchedEffect(updates) { updates?.check() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceMd.dp)) {
        Text("About Porchlight", style = MaterialTheme.typography.headlineSmall)
        Text("App version: ${BuildInfo.VERSION}")
        Text("Server version: ${server ?: status?.running ?: "not available"}")
        SecondaryAction("Check for updates", { updates?.check() }, enabled = updates != null)
        if (updates != null) UpdateStatus(updates)
        if (status != null) ServerUpdateCard(status)
        DesignPending("About and updates")
    }
}
@Composable
fun ServerUpdateCard(status: ServerRelease) {
    val latest = status.latest?.version ?: return
    if (!status.updateAvailable) return
    PorchlightCard(Modifier.fillMaxWidth()) {
        Text("Server $latest is available", style = MaterialTheme.typography.titleMedium)
        if (status.upgradeCommand != null) {
            Text("On the server, run:")
            SelectionContainer { Text("${status.upgradeCommand} $latest") }
        } else Text("Ask your server administrator to upgrade Porchlight.")
        status.checkedAt?.let { Text("Last checked: $it", style = MaterialTheme.typography.bodySmall) }
        status.lastError?.let { Text("Last check: $it", style = MaterialTheme.typography.bodySmall) }
        DesignPending("Server update card")
    }
}
@Composable
fun UpdateStatus(updates: UpdateController, required: Boolean = false) {
    val state by updates.state.collectAsState()
    UpdateStatusContent(state, required, { updates.retry() }, { updates.cancel() }, { updates.requestPermission() }, { updates.restart() })
}
@Composable
fun UpdateStatusContent(state: UpdateState, required: Boolean = false, update: () -> Unit = {}, cancel: () -> Unit = {},
    permission: () -> Unit = {}, restart: () -> Unit = {}) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        when (state) {
            UpdateState.Idle -> Text(if (required) "Ask your admin to upgrade the server" else "No client update is currently advertised.")
            is UpdateState.Available -> PrimaryAction(if (required) "Update now" else "Update to ${state.release.version}", update)
            is UpdateState.Downloading -> {
                val percent = if (state.total <= 0) 0 else (state.received * 100 / state.total).coerceIn(0, 100)
                Text("Downloading update: $percent%")
                LinearProgressIndicator(progress = { percent / 100f }, modifier = Modifier.fillMaxWidth())
                SecondaryAction("Cancel update", cancel)
            }
            UpdateState.Verifying -> { Text("Verifying update…"); SecondaryAction("Cancel update", cancel) }
            UpdateState.NeedsPermission -> {
                Text("Allow Porchlight to install this verified update. Android will still ask you to confirm the installation.")
                PrimaryAction("Allow app updates", permission)
                SecondaryAction("Cancel update", cancel)
            }
            UpdateState.Installing -> Text("Installing update… Confirm when Android asks, or wait for the desktop installer.")
            UpdateState.ReadyToRestart -> PrimaryAction("Restart to finish", restart)
            is UpdateState.Failed -> { Text(state.reason, color = MaterialTheme.colorScheme.error); SecondaryAction("Retry update", update) }
            is UpdateState.Unavailable -> Text(state.reason)
        }
    }
}
