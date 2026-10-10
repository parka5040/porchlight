package app.porchlight.shared.screens.settings

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.settings.*
import app.porchlight.shared.ui.*
import app.porchlight.shared.theme.PorchlightTokens

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SettingsScreen(settings: DeviceSettings, section: SettingsSection?, themeChanged: (ThemeMode) -> Unit,
    startChanged: (StartScreen) -> Unit, openSection: (SettingsSection) -> Unit, accountContent: (@Composable () -> Unit)? = null,
    services: app.porchlight.shared.AppServices? = null) {
    val cache = services?.data?.cache?.view?.collectAsState()?.value
    if (section == SettingsSection.About) { app.porchlight.shared.update.UpdateAbout(services?.updates); return }
    if (section in app.porchlight.shared.screens.planning.planningSections) {
        app.porchlight.shared.screens.planning.PlanningScreen(services, section!!)
        return
    }
    // Account owns its scrolling form; never nest it in an unbounded vertical scroller.
    if (section == SettingsSection.Account && accountContent != null) { accountContent(); return }
    if (section == SettingsSection.Notifications) { app.porchlight.shared.screens.inbox.NotificationPreferences(services); return }
    if (section == SettingsSection.Today) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp),
            verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceMd.dp)) {
            HouseholdSettingsPanel(services, today = true)
        }
        return
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceMd.dp)) {
        Text("Appearance", style = MaterialTheme.typography.headlineSmall)
        Text("Theme applies to this device. Your member color will sync to every device.")
        PorchlightCard(Modifier.fillMaxWidth()) {
            Text("Theme", style = MaterialTheme.typography.titleMedium)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
                ThemeMode.entries.forEach { mode ->
                    val label = if (mode == ThemeMode.System) "Follow system" else mode.name
                    ShellButton(label, "$label: theme", { themeChanged(mode) }, selected = settings.themeMode == mode)
                }
            }
            HouseholdSettingsPanel(services, today = false)
        }
        Text("Start screen", style = MaterialTheme.typography.headlineSmall)
        PorchlightCard(Modifier.fillMaxWidth()) {
            Text("What this device opens to")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
                StartScreen.entries.forEach { screen ->
                    ShellButton(if (screen == StartScreen.LastScreen) "Where I left off" else screen.name,
                        "${if (screen == StartScreen.LastScreen) "Where I left off" else screen.name}: start screen", { startChanged(screen) }, selected = settings.startScreen == screen)
                }
            }
        }
        Text("Settings sections", style = MaterialTheme.typography.titleMedium)
        FlowRow {
            (listOf(SettingsSection.About, SettingsSection.Today, SettingsSection.Notifications, SettingsSection.Account, SettingsSection.CalendarData, SettingsSection.Availability, SettingsSection.Planning, SettingsSection.Export) +
                if (cache?.let { view -> view.members.find { it.id == view.settings?.memberId }?.role == "super_admin" } == true) listOf(SettingsSection.Devices) else emptyList()).forEach { s ->
                ShellButton(if (s == SettingsSection.Account) "Account and devices" else when (s) { SettingsSection.CalendarData -> "Calendar data"; SettingsSection.Devices -> "Admin devices"; else -> s.name },
                    "${if (s == SettingsSection.Account) "Account and devices" else when (s) { SettingsSection.CalendarData -> "Calendar data"; SettingsSection.Devices -> "Admin devices"; else -> s.name }}: open settings", { openSection(s) }, selected = section == s)
            }
        }
        if (section != null && section !in listOf(SettingsSection.Appearance, SettingsSection.StartScreen)) {
            // Today returned above; Notifications and the unsigned-in Account fallback need design passes.
            PorchlightCard(Modifier.fillMaxWidth()) {
                Text(if (section == SettingsSection.Account) "Account and devices" else section.name,
                    style = MaterialTheme.typography.titleMedium)
                Text(when (section) {
                    SettingsSection.Notifications -> "Category toggles, quiet hours and default reminders sync to every device."
                    else -> "Password and enrolled devices appear after sign-in."
                })
                DesignPending("Settings ${section.name}")
            }
        }
    }
}
