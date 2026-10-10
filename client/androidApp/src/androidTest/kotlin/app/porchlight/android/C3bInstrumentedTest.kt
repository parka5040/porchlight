package app.porchlight.android

import androidx.activity.ComponentActivity
import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.porchlight.shared.api.models.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.CalendarContent
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.ui.MemberChip
import kotlinx.datetime.*
import org.junit.Rule
import org.junit.Test
import org.junit.Assert.*
import org.junit.runner.RunWith

/** Offline deterministic key-screen equivalents. Discovered on both API35/36; not ControllerOnly. */
@RunWith(AndroidJUnit4::class)
class C3bInstrumentedTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @Test fun mobileCalendarDayWeekAgendaAtDoubleScaleInBothThemes() {
        var mode by mutableStateOf(ThemeMode.Evening)
        var route by mutableStateOf(AppRoute.Calendar(CalendarView.Day, "2026-03-08"))
        val row = WireJson.decodeFromString<ItemRead>("""{"id":"i","household_id":"h","title":"Meds","notes":"","priority":0,"owner_id":"self","visibility":"household","kind":"appointment","time_mode":"floating","all_day":false,"buffer_before_min":0,"buffer_after_min":0,"missed_policy":"carry_over","version":1,"created_at":"2026-03-08T07:00:00Z","updated_at":"2026-03-08T07:00:00Z","assignees":[],"viewers":[],"reminders":[],"capabilities":["view"],"pending_derivation":true,"agenda":{"household_id":"h","member_id":"self","occurrence_id":"o","item_id":"i","starts_at":"2026-03-08T07:00:00Z","derived_at":"2026-03-08T07:00:00Z"}}""")
        compose.setContent { CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) {
            PorchlightTheme(mode, false) { CalendarContent(route, LocalDate(2026,3,8), TimeZone.of("America/New_York"), "self", listOf(row), emptyList(), true, false, null, { route = it }, {}, {}) }
        } }
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            compose.runOnIdle { mode = theme; route = AppRoute.Calendar(CalendarView.Day, "2026-03-08") }
            compose.onNodeWithText("Floating · 03:00").performScrollTo().assertIsDisplayed()
            compose.onNodeWithText("Pending derivation").assertExists()
            compose.onNodeWithContentDescription("Week").performScrollTo().performClick()
            compose.onNodeWithContentDescription("Week").assertIsSelected()
            compose.onNodeWithContentDescription("Agenda").performScrollTo().performClick()
            compose.onNodeWithContentDescription("Agenda").assertIsSelected()
            compose.onNodeWithContentDescription("Month").assertDoesNotExist()
            compose.onNodeWithContentDescription("Next").performScrollTo().assertHeightIsAtLeast(48.dp).assertWidthIsAtLeast(48.dp)
        }
    }
    @Test fun settingsControlsInitialsAndRangeAtDoubleScaleBothThemes() {
        var mode by mutableStateOf(ThemeMode.Evening)
        val base = WireJson.decodeFromString<SettingsRead>("""{"member_id":"self","version":3,"settings":{"today_sections":["today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}}""")
        compose.setContent { CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) {
            PorchlightTheme(mode, false) { Column(Modifier.verticalScroll(rememberScrollState())) {
                MemberChip("Sam Rivera", 0)
                ColorChoices("#FABD2F", false, {})
                TodaySettingsForm(base, false, {})
            } }
        } }
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            compose.runOnIdle { mode = theme }
            compose.onNodeWithText("SR", useUnmergedTree = true).assertExists()
            compose.onNodeWithContentDescription("Blue").performScrollTo().assertHasClickAction()
            compose.onNodeWithContentDescription("This week days").performScrollTo().performTextReplacement("14")
            compose.onNodeWithContentDescription("Save Today settings").performScrollTo().assertIsDisplayed().assertHeightIsAtLeast(48.dp)
        }
    }
}
