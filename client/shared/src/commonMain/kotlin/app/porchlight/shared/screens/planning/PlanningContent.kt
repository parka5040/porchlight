package app.porchlight.shared.screens.planning

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.ui.*

// DESIGN-PENDING: material forms derived from specification; planning cards follow DesktopCalendar panels.
@Composable private fun PlanningForm(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(title, style = MaterialTheme.typography.headlineSmall)
        content()
    }
}
@Composable private fun Field(label: String, value: String, changed: (String) -> Unit, password: Boolean = false) {
    OutlinedTextField(value, changed, label = { Text(label) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
        visualTransformation = if (password) PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
        keyboardOptions = if (password) KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false) else KeyboardOptions.Default,
        singleLine = true)
}
@Composable fun ImportContent(summary: ImportSummary? = null, filename: String? = null, busy: Boolean = false,
    choose: () -> Unit = {}, commit: () -> Unit = {}, cancel: () -> Unit = {}, feedback: @Composable () -> Unit = {}, retry: () -> Unit = {}) {
    PlanningForm("Import ICS calendar") {
        Text("Choose an .ics file (up to 5 MiB). Preview is not a save; cancel leaves your calendar unchanged.")
        filename?.let { Text(it) }
        ShellButton("Choose ICS file", "Choose ICS file", choose, enabled = !busy)
        if (filename != null && summary == null) ShellButton("Retry preview", "Retry preview", retry, enabled = !busy)
        summary?.let { value ->
            importSummaryLines(value).forEach { Text(it) }
            if (value.preview) {
                ShellButton("Commit import", "Commit import", commit, enabled = !busy)
                ShellButton("Cancel preview", "Cancel preview", cancel, enabled = !busy)
            } else Text("Imported calendar saved. Editable imports and read-only subscribed feeds have different capabilities.")
        }
        feedback()
    }
}
@Composable fun FeedContent(rows: List<Source> = emptyList(), name: String = "", url: String = "",
    nameChanged: (String) -> Unit = {}, urlChanged: (String) -> Unit = {}, add: () -> Unit = {}, refresh: (Source) -> Unit = {},
    remove: (Source) -> Unit = {}, reload: () -> Unit = {}, busy: Boolean = false, feedback: @Composable () -> Unit = {},
    members: List<MemberRead> = emptyList(), target: String? = null, targetChanged: (String?) -> Unit = {},
    interval: String = "86400", intervalChanged: (String) -> Unit = {}, visibility: SourceVisibility = SourceVisibility.household,
    visibilityChanged: (SourceVisibility) -> Unit = {}, enabled: Boolean = true, enabledChanged: (Boolean) -> Unit = {},
    editing: Boolean = false, edit: (Source) -> Unit = {}, cancelEdit: () -> Unit = {}, zone: String = "UTC") {
    PlanningForm("Subscribed feeds") {
        Text("Super admin · choose household or member (including dependents). Feed items are read-only; server checks URLs to protect your network.")
        Row { ShellButton("Household", "Feed target household", { targetChanged(null) }, selected = target == null, enabled = !busy) }
        Column { members.forEach { m -> ShellButton(m.displayName, "Feed target ${m.displayName}", { targetChanged(m.id) }, selected = target == m.id, enabled = !busy) } }
        Field("Refresh interval (seconds, 60–2592000)", interval, intervalChanged)
        Row { ShellButton("Private", "Feed visibility private", { visibilityChanged(SourceVisibility.private) }, selected = visibility == SourceVisibility.private, enabled = !busy)
            ShellButton("Household visible", "Feed visibility household", { visibilityChanged(SourceVisibility.household) }, selected = visibility == SourceVisibility.household, enabled = !busy) }
        Row { Checkbox(enabled, { enabledChanged(it) }, enabled = !busy); Text("Polling enabled") }
        Field("Feed name", name, nameChanged)
        Field("Calendar URL (HTTPS)", url, urlChanged)
        ShellButton(if (editing) "Save feed changes" else "Add feed", if (editing) "Save feed changes" else "Add feed", add, enabled = !busy)
        if (editing) ShellButton("Cancel feed edit", onClick = cancelEdit, enabled = !busy)
        ShellButton("Refresh feed list", "Refresh feed list", reload, enabled = !busy)
        rows.forEach { row -> PorchlightCard(Modifier.fillMaxWidth()) {
            Text(row.name, style = MaterialTheme.typography.titleMedium)
            Text(row.url ?: "URL unavailable")
            Text("Target: ${members.find { it.id == row.targetMemberId }?.displayName ?: if (row.targetMemberId == null) "Household" else "Member"} · ${if (row.visibility == SourceVisibility.private) "Private" else "Household visible"}")
            Text("${if (row.enabled) "Enabled" else "Disabled"} · every ${row.refreshSeconds} seconds")
            Text("${row.lastStatus?.replace('_', ' ')?.replaceFirstChar { it.uppercase() } ?: "Not fetched"} · last fetch ${optionalPlanningTime(row.lastFetchAt, zone)}")
            Text("Failures: ${row.failureCount} · next fetch/backoff ${formatPlanningTime(row.nextFetchAt, zone)}")
            row.lastError?.let { Text(it) }
            row.lastWarnings.forEach { Text("${it.code}: ${it.message}") }
            if (row.deletedAt == null) {
                ShellButton("Edit feed", "Edit feed ${row.name}", { edit(row) }, enabled = !busy)
                ShellButton("Refresh now", "Refresh feed ${row.name}", { refresh(row) }, enabled = !busy)
                ShellButton("Remove feed", "Remove feed ${row.name}", { remove(row) }, enabled = !busy)
            }
        } }
        Text("Removing a feed disables polling and soft-deletes its imported events. History is preserved, not purged. Re-adding the same URL creates a new source.")
        feedback()
    }
}
@OptIn(ExperimentalLayoutApi::class)
@Composable fun AvailabilityContent(windows: List<Window> = emptyList(), changed: (List<Window>) -> Unit = {}, save: () -> Unit = {},
    reload: () -> Unit = {}, members: List<MemberRead> = emptyList(), member: String? = null, select: (String) -> Unit = {},
    busy: Boolean = false, feedback: @Composable () -> Unit = {}, zone: String = "UTC") {
    PlanningForm("Availability") {
        Text("Member calendar zone: $zone")
        Text("Weekly local-time windows. No windows means load is not tracked; missing weekdays otherwise have zero capacity. Overnight windows are allowed; overlaps are unioned.")
        FlowRow { members.forEach { m -> ShellButton(m.displayName, "Edit availability for ${m.displayName}", { select(m.id) }, selected = member == m.id, enabled = !busy) } }
        windows.forEachIndexed { i, row -> PorchlightCard(Modifier.fillMaxWidth()) {
            val days = listOf("Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday")
            Text(days.getOrElse(row.weekday) { "Invalid weekday" })
            FlowRow { days.forEachIndexed { day, label -> ShellButton(label.take(3), "Window ${i + 1} $label", { changed(windows.toMutableList().also { it[i] = row.copy(weekday = day) }) }, selected = row.weekday == day, enabled = !busy) } }
            Field("Window ${i + 1} start HH:mm:ss", row.start, { value -> changed(windows.toMutableList().also { it[i] = row.copy(start = value) }) })
            Field("Window ${i + 1} end HH:mm:ss", row.end, { value -> changed(windows.toMutableList().also { it[i] = row.copy(end = value) }) })
            ShellButton("Remove window", "Remove window ${i + 1}", { changed(windows.filterIndexed { index, _ -> index != i }) }, enabled = !busy)
        } }
        ShellButton("Add weekly window", "Add weekly window", { changed(windows + Window(0, "09:00:00", "17:00:00")) }, enabled = !busy)
        ShellButton("Save availability", "Save availability", save, enabled = !busy)
        ShellButton("Refetch availability", "Refetch availability", reload, enabled = !busy)
        Text("Load capacity comes from these windows. Date exceptions and independent load limits are not supported by this server version.")
        feedback()
    }
}
@Composable fun PlanningCardsContent(rows: List<SuggestionResponse> = emptyList(), title: (String) -> String = { it },
    request: () -> Unit = {}, accept: (SuggestionResponse) -> Unit = {}, dismiss: (SuggestionResponse) -> Unit = {},
    candidates: CandidatesResponse? = null, candidateRequest: () -> Unit = {}, candidateName: (String) -> String = { it },
    unplaced: List<Unplaced> = emptyList(), busy: Boolean = false, feedback: @Composable () -> Unit = {},
    reload: () -> Unit = {}, memberName: (String) -> String = { "Member" }, zone: (String) -> String = { "UTC" }, occurrenceName: (String) -> String = { "Unplaced work" }, expired: (SuggestionResponse) -> Boolean = { false }) {
    PlanningForm("Suggested times") {
        Text("Earliest deadline first · proposals never move work automatically. Accept schedules only this occurrence; conflict warnings do not block saving.")
        ShellButton("Request suggestions", "Request suggestions", request, enabled = !busy)
        ShellButton("Refetch suggestions", "Refetch suggestions", reload, enabled = !busy)
        rows.filter { it.status == SuggestionStatus.proposed }.forEach { row -> PorchlightCard(Modifier.fillMaxWidth()) {
            Text(title(row.itemId), style = MaterialTheme.typography.titleMedium)
            Text("${memberName(row.memberId)} · ${formatPlanningTime(row.proposedStart, zone(row.memberId))} – ${formatPlanningTime(row.proposedEnd, zone(row.memberId))}")
            if (expired(row)) Text("Proposal expired or no longer applies.")
            ShellButton("Accept", "Accept suggestion ${row.id}", { accept(row) }, enabled = !busy && !expired(row) && row.occurrenceVersion != null)
            ShellButton("Dismiss", "Dismiss suggestion ${row.id}", { dismiss(row) }, enabled = !busy)
        } }
        if (rows.none { it.status == SuggestionStatus.proposed }) Text("No proposed times. Add availability and estimates/deadlines before requesting suggestions.")
        unplaced.forEach { Text("${occurrenceName(it.occurrenceId)}: ${unplacedReason(it.reason)}") }
        ShellButton("Dependent assignee candidates", "Dependent assignee candidates", candidateRequest, enabled = !busy)
        candidates?.let { result ->
            if (result.pendingDerivation) Text("Planning calculation pending · refetch after derivation")
            result.candidates.forEach { Text("${candidateName(it.memberId)} · ${candidateReason(it.reason)}") }
        }
        feedback()
    }
}
@Composable fun ExportContent(admin: Boolean = false, member: () -> Unit = {}, household: () -> Unit = {},
    busy: Boolean = false, feedback: @Composable () -> Unit = {}) {
    PlanningForm("Export data") {
        Text("Your export contains the data you can see and appointments_ics, which can be saved as appointments.ics. Protect the downloaded file: it contains household data, not login secrets.")
        ShellButton("Export my data", "Export my data", member, enabled = !busy)
        if (admin) ShellButton("Export household", "Export household", household, enabled = !busy)
        Text("Household export requires password step-up and includes attachments. Incomplete downloads are rejected before publishing.")
        feedback()
    }
}
@Composable fun DeviceAdminContent(rows: List<Device> = emptyList(), memberName: (String) -> String = { it },
    reload: () -> Unit = {}, revoke: (Device) -> Unit = {}, busy: Boolean = false, feedback: @Composable () -> Unit = {}, zone: String = "UTC") {
    PlanningForm("Admin devices") {
        Text("Super admin · revocation rejects every certificate for the device and closes its sessions. Own-device logout is in Settings → Account.")
        ShellButton("Refresh devices", "Refresh devices", reload, enabled = !busy)
        rows.forEach { row -> PorchlightCard(Modifier.fillMaxWidth()) {
            Text("${memberName(row.memberId)} · ${row.label}", style = MaterialTheme.typography.titleMedium)
            Text("${row.platform.replaceFirstChar { it.uppercase() }} · last seen ${optionalPlanningTime(row.lastSeenAt, zone)}${if (row.current) " · this device" else ""}")
            Text("Certificate expires: ${row.certificateExpiresAt?.let { formatPlanningTime(it, zone) } ?: "No active certificate"}")
            if (row.revokedAt == null) ShellButton("Revoke device", "Revoke device ${row.label}", { revoke(row) }, enabled = !busy && !row.current)
            else Text("Revoked ${optionalPlanningTime(row.revokedAt, zone)}")
        } }
        feedback()
    }
}
@Composable fun PlanningPasswordDialog(action: String, busy: Boolean, confirm: (String) -> Unit, cancel: () -> Unit, problem: String? = null) {
    var password by remember { mutableStateOf("") }
    AlertDialog(onDismissRequest = { if (!busy) cancel() }, title = { Text("Confirm $action") },
        text = { Column { Text("Re-enter your password for this sensitive operation."); problem?.let { Text(it, color = MaterialTheme.colorScheme.error) }; Field("Password", password, { password = it }, password = true) } },
        confirmButton = { ShellButton("Confirm", "Confirm $action", { val value = password; password = ""; confirm(value) }, enabled = !busy) },
        dismissButton = { ShellButton("Cancel", "Cancel $action", cancel, enabled = !busy) })
}
