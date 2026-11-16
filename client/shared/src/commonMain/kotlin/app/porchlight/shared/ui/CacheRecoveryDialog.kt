package app.porchlight.shared.ui

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Modifier

@Composable
fun CacheRecoveryDialog(onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Clear local cache?") },
        text = { Text("This clears downloaded household data and attachment files, then reloads authorised data. Your device credentials, settings, item and comment drafts, and pending requests are kept. Existing-resource edits stay hidden until access is checked again; edits for resources no longer visible are discarded. Other unsent screen fields (including list and planning forms) may reset; copy them before continuing.", Modifier.verticalScroll(rememberScrollState())) },
        confirmButton = { ShellButton("Confirm clear local cache", onClick = onConfirm) },
        dismissButton = { ShellButton("Keep local cache", onClick = onDismiss) })
}
