package app.porchlight.shared.backups

import app.porchlight.shared.theme.PorchlightTokens

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

// DESIGN-PENDING: desktop super-admin backup status, not an approved mockup.
@Composable
fun BackupPanel(backups: DesktopBackups) {
    val view by backups.state.collectAsState()
    val scope = rememberCoroutineScope()
    if (!view.allowed) {
        if (view.cantReachHome) Text("Can't reach home", modifier = Modifier.padding(PorchlightTokens.spaceMd.dp), color = MaterialTheme.colorScheme.error)
        return
    }
    Column(Modifier.fillMaxWidth().heightIn(max = PorchlightTokens.backupPanelMaxHeight.dp).verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        Text("Backups · DESIGN-PENDING", style = MaterialTheme.typography.titleLarge)
        Text("Last success: ${view.status?.lastSuccess ?: "not loaded / no backup yet"}")
        Text("Next server run: ${view.status?.nextDue ?: "not scheduled yet"}")
        if (view.status?.lastError != null) Text("Last server run failed. Inspect private server diagnostics.")
        Text("Local encrypted bundle: ${view.localBundle ?: "none this session"}")
        view.unreachableSince?.let { Text("Can't reach home since $it") }
        view.notice?.let { Text(it) }
        view.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(enabled = !view.pulling, onClick = {
            scope.launch { backups.pull() }
        }, modifier = Modifier.heightIn(min = PorchlightTokens.touchTarget.dp)) { Text(if (view.pulling) "Pulling…" else "Refresh and pull newest") }
        Text("Encrypted bundles only. Keep the recovery identity in your personal backup, never on the server.")
    }
}
