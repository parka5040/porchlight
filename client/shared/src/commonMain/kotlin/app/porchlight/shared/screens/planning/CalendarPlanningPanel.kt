package app.porchlight.shared.screens.planning

import androidx.compose.material3.*
import androidx.compose.runtime.*
import app.porchlight.shared.AppServices
import app.porchlight.shared.api.models.SuggestionStatus
import app.porchlight.shared.transport.ApiProblem
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import kotlinx.coroutines.launch

@Composable fun CalendarPlanningPanel(services: AppServices?, cache: CacheView, open: (SettingsSection) -> Unit) {
    val data = services?.data ?: return
    val member = cache.settings?.memberId ?: return
    val writer = screenWrites("c6-calendar-planning:$member")
    val state by writer.state.collectAsState()
    val scope = applicationWriteScope()
    var notice by remember { mutableStateOf<String?>(null) }
    fun submit(row: app.porchlight.shared.api.models.SuggestionResponse? = null, op: suspend (String) -> WriteResult<kotlinx.serialization.json.JsonElement>) {
        scope.launch { writer.submit { key ->
            try { op(key) } catch (p: ApiProblem) {
                if (row != null && p.status == 422) { services.screenSession?.rejectSuggestion(row); notice = "Proposal expired or no longer applies; suggestions refetched." } else notice = p.detail
                throw p
            }
        } }
    }
    val zone = planningMemberZone(cache.members.find { it.id == member })
    Text("Suggested times", style = MaterialTheme.typography.headlineSmall)
    Text("EDF proposals · nothing moves automatically")
    cache.suggestions.filter { it.memberId == member && it.status == SuggestionStatus.proposed }.take(3).forEach { row -> PorchlightCard {
        Text(cache.items.find { it.id == row.itemId }?.title ?: "Scheduled work")
        Text("${cache.members.find { it.id == row.memberId }?.displayName ?: "Member"} · ${formatPlanningTime(row.proposedStart, zone)} – ${formatPlanningTime(row.proposedEnd, zone)}")
        val expired = suggestionRejectionKey(row) in services.screenSession?.rejectedSuggestions.orEmpty()
        if (expired) Text("Proposal expired or no longer applies.")
        ShellButton("Accept", "Accept suggestion ${row.id}", { submit(row) { data.planning.accept(row, it).asScreenResult() } }, enabled = !state.busy && !expired && row.occurrenceVersion != null)
        ShellButton("Dismiss", "Dismiss suggestion ${row.id}", { submit { data.planning.dismiss(row, it).asScreenResult() } }, enabled = !state.busy)
    } }
    ShellButton("Request suggestions", onClick = { scope.launch { writer.submit { data.planning.suggest(member, it).asScreenResult() } } }, enabled = !state.busy)
    ShellButton("Refetch suggestions", onClick = { scope.launch {
        try { data.planning.refresh(member); notice = "Suggestions refreshed" }
        catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (_: Exception) { notice = "Unable to refresh suggestions" }
    } }, enabled = !state.busy)
    notice?.let { Text(it) }
    ShellButton("All suggestions and dependent candidates", onClick = { open(SettingsSection.Planning) })
    ShellButton("Edit availability", onClick = { open(SettingsSection.Availability) })
    WriteStatus(writer)
}
