package app.porchlight.shared.screens.settings

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.Alignment
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.shape.CircleShape
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.theme.PorchlightTokens
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.AppServices
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.ui.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@Composable fun HouseholdSettingsPanel(services: AppServices?, today: Boolean) {
    val data = services?.data
    val cache = data?.cache?.view?.collectAsState()?.value ?: CacheView()
    val writer = services?.settingsWrites
    val state = writer?.state?.collectAsState()?.value ?: SettingsWriteState()
    val fallbackScope = rememberCoroutineScope()
    val scope = services?.screenSession?.writeScope ?: fallbackScope
    var error by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(false) }
    var refresh by remember { mutableIntStateOf(0) }
    LaunchedEffect(data, refresh) {
        if (data == null) return@LaunchedEffect
        writer?.checkIdentity()
        loading = true
        try { data.household.refreshSettings(); data.household.refreshMembers(); error = false }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { error = true }
        finally { loading = false }
    }
    if (loading) Text("Refreshing settings…")
    if (error) ErrorState("Can't refresh settings — showing saved values.", { refresh++ })
    val blocked = writer == null || writer.unresolved
    if (today) {
        cache.settings?.let { base -> TodaySettingsForm(base, blocked, { body -> scope.launch { writer?.saveToday(base, body) } }, state.fieldErrors) }
            ?: Text("Sign in and sync to edit Today settings.")
    } else {
        Text("Your color", style = MaterialTheme.typography.titleMedium)
        val member = cache.members.find { it.id == cache.settings?.memberId }
        if (member == null) Text("Sign in and sync to edit your member color.")
        else {
            MemberChip(member)
            if (member.pendingDerivation) StatusPill("Pending derivation")
            ColorChoices(member.color, blocked) { color -> scope.launch { writer?.saveColor(member, color) } }
            state.fieldErrors["color"]?.let { Text(it, color = LocalPorchlightPalette.current.errorText) }
        }
    }
    val inlineFields = if (today && cache.settings != null) setOf("this_week_days", "scope", "today_sections", "pinned_list_id")
        else if (!today && cache.members.any { it.id == cache.settings?.memberId }) setOf("color") else emptySet()
    SettingsWriteFeedback(state.copy(fieldErrors = state.fieldErrors.filterKeys { it !in inlineFields }),
        { scope.launch { writer?.retry() } }, { scope.launch { writer?.reapply() } }, { writer?.discard(); refresh++ },
        retryAllowed = state.fieldErrors.isEmpty())
}
@OptIn(ExperimentalLayoutApi::class)
@Composable fun ColorChoices(selected: String, blocked: Boolean, choose: (String) -> Unit) {
    val p = LocalPorchlightPalette.current
    val textMeasurer = rememberTextMeasurer()
    val labelStyle = MaterialTheme.typography.labelLarge
    // Size every column for the longest label at the current font scale, not for its selection.
    val labelWidth = with(LocalDensity.current) {
        memberColorChoices.keys.maxOf { textMeasurer.measure(it, labelStyle).size.width }.toDp()
    } + (PorchlightTokens.selectionStroke / 2).dp // pixel-rounding slack: at fractional densities (e.g. 420dpi) padding rounds up by a pixel
    // The ring is the non-colour selection cue, so labels need no duplicated check slots.
    // Use this label's own padding, not a copy of ShellButton's internal chrome.
    val columnWidth = maxOf(PorchlightTokens.touchTarget.dp, labelWidth + (2 * PorchlightTokens.spaceInset).dp)
    FlowRow(Modifier.selectableGroup(), horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceXs.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceXs.dp)) {
        memberColorChoices.forEach { (name, color) ->
            val isSelected = selected.equals(color, ignoreCase = true)
            Column(Modifier.width(columnWidth).heightIn(min = PorchlightTokens.touchTarget.dp)
                .selectable(isSelected, enabled = !blocked, role = Role.RadioButton) { choose(color) }
                .semantics(mergeDescendants = true) { contentDescription = name },
                horizontalAlignment = Alignment.CenterHorizontally) {
                // Fixed 36dp box: 24dp fill, 4dp surface gap, 2dp high-contrast outer ring.
                // The gap makes selection independent of any member fill, including Yellow.
                Box(Modifier.size((PorchlightTokens.iconSize + PorchlightTokens.spaceInset).dp).testTag("color-ring-box-$name"), contentAlignment = Alignment.Center) {
                    if (isSelected) Surface(Modifier.fillMaxSize().testTag("color-ring-$name"),
                        color = p.surface, shape = CircleShape,
                        border = BorderStroke(PorchlightTokens.selectionStroke.dp, p.heading)) {}
                    Surface(Modifier.size(PorchlightTokens.iconSize.dp).testTag("color-dot-$name"), color = memberColor(color), shape = CircleShape) {}
                }
                Surface(Modifier.fillMaxWidth().heightIn(min = PorchlightTokens.touchTarget.dp),
                    color = if (isSelected) p.selectedSurface else androidx.compose.ui.graphics.Color.Transparent,
                    shape = MaterialTheme.shapes.small,
                    border = if (isSelected) BorderStroke(PorchlightTokens.selectionStroke.dp, p.primary) else null) {
                    Box(Modifier.padding(horizontal = PorchlightTokens.spaceInset.dp, vertical = PorchlightTokens.spaceSm.dp),
                        contentAlignment = Alignment.Center) {
                        Text(name, color = if (isSelected) p.primaryText else p.onSurface,
                            style = labelStyle, textAlign = TextAlign.Center, maxLines = 1, softWrap = false)
                    }
                }
            }
        }
    }
    if (blocked) Text("Finish the pending save before changing your color.")
}
@OptIn(ExperimentalLayoutApi::class)
@Composable fun TodaySettingsForm(base: SettingsRead, blocked: Boolean, save: (TodaySettings) -> Unit, errors: Map<String, String> = emptyMap()) {
    var draft by remember(base.memberId, base.version) { mutableStateOf(TodayDraft(base.settings)) }
    Text("Today settings", style = MaterialTheme.typography.titleMedium)
    Text("Sections, order, filter and week range sync to every device. Save explicitly when ready.")
    draft.order.forEach { section ->
        val label = sectionLabel(section)
        PorchlightCard(Modifier.fillMaxWidth()) {
            ShellButton(label, "${if (section in draft.enabled) "Disable" else "Enable"} $label", { if (!blocked) draft = draft.toggle(section) }, selected = section in draft.enabled, enabled = !blocked && (section != "pinned_list" || base.settings.pinnedListId != null || section in draft.enabled))
            FlowRow {
                if (draft.order.indexOf(section) > 0) ShellButton("Move $label up", "Move $label up", { if (!blocked) draft = draft.move(section, -1) }, enabled = !blocked)
                if (draft.order.indexOf(section) < draft.order.lastIndex) ShellButton("Move $label down", "Move $label down", { if (!blocked) draft = draft.move(section, 1) }, enabled = !blocked)
            }
            if (section == "pinned_list") {
                Text("Pinned list ${base.settings.pinnedListId ?: "not selected"} · list selection arrives with Lists.")
                errors["pinned_list_id"]?.let { Text(it, color = LocalPorchlightPalette.current.errorText) }
            }
        }
    }
    errors["today_sections"]?.let { Text(it, color = LocalPorchlightPalette.current.errorText) }
    FlowRow {
        ShellButton("Mine only", "Mine only", { if (!blocked) draft = draft.withScope("mine") }, selected = draft.scope == "mine", enabled = !blocked)
        ShellButton("Whole household", "Whole household", { if (!blocked) draft = draft.withScope("household") }, selected = draft.scope == "household", enabled = !blocked)
    }
    errors["scope"]?.let { Text(it, color = LocalPorchlightPalette.current.errorText) }
    val daysError = errors["this_week_days"] ?: if (draft.validatedValue() == null) "This week days must be a whole number from 1 to 14." else null
    OutlinedTextField(draft.days, { draft = draft.withDays(it) }, label = { Text("This week days (1–14)") }, enabled = !blocked,
        isError = daysError != null, supportingText = { daysError?.let { Text(it, color = LocalPorchlightPalette.current.errorText) } },
        modifier = Modifier.fillMaxWidth().heightIn(min = PorchlightTokens.touchTarget.dp).semantics { contentDescription = "This week days" })
    if (!blocked && draft.validatedValue() != null) ShellButton("Save Today settings", "Save Today settings", { save(draft.value()) })
    if (blocked) Text("Finish the pending save before editing settings.")
}
@Composable fun SettingsWriteFeedback(state: SettingsWriteState, retry: () -> Unit, reapply: () -> Unit, discard: () -> Unit,
    retryAllowed: Boolean = state.fieldErrors.isEmpty()) {
    when (state.status) {
        SettingsWriteStatus.Idle -> Unit
        SettingsWriteStatus.Saving -> Text("Saving settings…")
        SettingsWriteStatus.Saved -> Text(if (state.refreshPending) "Saved — refresh pending." else "Settings saved.")
        SettingsWriteStatus.Conflict -> PorchlightCard {
            DesignPending("Settings compare")
            Text("Settings changed on another device. Compare before choosing. Reapply keeps the current pinned list.")
            Text("Yours: ${state.yours}"); Text("Current: ${state.current}")
            ShellButton("Reapply mine", "Reapply mine", reapply)
            ShellButton("Discard mine", "Discard mine", discard)
        }
        SettingsWriteStatus.Ambiguous -> PorchlightCard {
            Text("Save outcome unknown. Unlock or reconnect, then retry the identical request. Nothing queued offline.")
            ShellButton("Retry same save", "Retry same save", retry)
        }
        SettingsWriteStatus.Error -> PorchlightCard {
            Text("Settings could not be saved.")
            state.fieldErrors.forEach { (field, message) -> Text("$field: $message") }
            if (retryAllowed) ShellButton("Retry same save", "Retry same save", retry)
            ShellButton("Edit settings", "Edit settings", discard)
        }
    }
}
