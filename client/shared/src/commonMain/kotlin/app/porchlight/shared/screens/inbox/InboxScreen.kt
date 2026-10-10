package app.porchlight.shared.screens.inbox

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.Notification
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.*
import kotlinx.datetime.*
import kotlin.time.Instant

 data class InboxGroup(val title: String, val rows: List<Notification>)
fun inboxGroups(rows: List<Notification>, at: Instant, tzid: String): List<InboxGroup> {
    val zone = runCatching { TimeZone.of(tzid) }.getOrDefault(TimeZone.UTC)
    val today = at.toLocalDateTime(zone).date
    return rows.sortedByDescending { it.createdAt }.groupBy {
        val day = it.createdAt.toLocalDateTime(zone).date
        when { day >= today -> "Today"; day == today - DatePeriod(days = 1) -> "Yesterday"
            day >= today - DatePeriod(days = 7) -> "This week"; else -> "Earlier" }
    }.map { InboxGroup(it.key, it.value) }
}
/** MobileInbox structure, safe server templates rather than interpolating item titles. Desktop DESIGN-PENDING. */
@Composable fun InboxContent(groups: List<InboxGroup>, read: (String) -> Unit, readAll: () -> Unit,
    open: (Notification) -> Unit, preferences: () -> Unit) {
    LazyColumn(Modifier.fillMaxSize().padding(16.dp).testTag("inbox"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item { FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ShellButton("Mark all read", onClick = readAll)
            ShellButton("Notification preferences", onClick = preferences)
        } }
        if (groups.isEmpty()) item { EmptyState("All caught up", "No household notifications yet.") }
        groups.forEach { group ->
            item(key = "group-${group.title}") { Text(group.title, style = MaterialTheme.typography.titleMedium) }
            items(group.rows, key = { it.id }) { row -> PorchlightCard(Modifier.fillMaxWidth().testTag("notification-${row.id}")) {
                Text(row.summary.title, style = MaterialTheme.typography.titleMedium)
                Text(row.summary.body)
                Text("${notificationTopicLabel(row.summary.category)}" + if (row.readAt == null) " · Unread" else " · Read")
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (row.readAt == null) ShellButton("Got it", description = "Mark ${row.summary.title} read", onClick = { read(row.id) })
                    if (row.itemId != null) ShellButton("Open item", description = "Open item for ${row.summary.title}", onClick = { open(row) })
                }
            } }
        }
    }
}
@Composable fun InboxScreen(navigate: (AppRoute) -> Unit = {}) {
    val services = LocalScreenServices.current
    val data = services?.data
    if (data == null) { DataStatus(null); return }
    val view by data.cache.view.collectAsState()
    val scope = applicationWriteScope()
    val writer = screenWrites("inbox")
    val member = view.members.firstOrNull { it.id == view.settings?.memberId }
    var at by remember { mutableStateOf(services.clock.now()) }
    LaunchedEffect(services) { while (isActive) { at = services.clock.now(); delay(60_000) } }
    Column(Modifier.fillMaxSize()) {
        DataStatus(data); WriteStatus(writer)
        InboxContent(inboxGroups(view.notifications, at, member?.tzid ?: "UTC"),
            { id -> scope.launch { writer.submit { data.notifications.read(id, it).feedback() } } },
            { scope.launch { writer.submit { data.notifications.readAll(it).feedback() } } },
            { row -> row.itemId?.let { navigate(AppRoute.Item(it, row.occurrenceId)) } },
            { navigate(AppRoute.Settings(SettingsSection.Notifications)) })
    }
}
@Composable fun ReachabilityBanner(since: Instant?, zone: String = TimeZone.currentSystemDefault().id, retry: () -> Unit) {
    if (since != null) PorchlightCard(Modifier.fillMaxWidth().testTag("unreachable-banner")) {
        Text("Can't reach home since ${reachabilityTime(since, zone)}. Saved data and local reminders remain available.")
        ShellButton("Retry connection", onClick = retry)
    }
}

fun reachabilityTime(since: Instant, zone: String): String {
    val local = since.toLocalDateTime(runCatching { TimeZone.of(zone) }.getOrDefault(TimeZone.currentSystemDefault()))
    return "${local.date} ${local.hour.toString().padStart(2, '0')}:${local.minute.toString().padStart(2, '0')} ($zone)"
}
fun notificationTopicLabel(topic: String): String = when (topic) {
    "comments" -> "Comments"
    "assignments", "assigned" -> "Assignments"
    "status", "completed", "status_changed" -> "Status changes"
    "conflicts" -> "Calendar conflicts"
    "dependents", "dependent_appointments" -> "Dependents' appointments"
    "reminders" -> "Reminders"
    "digest" -> "Morning digest"
    "agent", "agent_changes" -> "Agent changes"
    else -> "Household activity"
}
