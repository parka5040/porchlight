package app.porchlight.android

import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import kotlin.time.Instant

/** Deterministic API35/36 content tests: no live trust, clock, sleeps or UI-thread runBlocking. */
class C3aScreensInstrumentedTest {
    @get:Rule val compose = createComposeRule()
    private val themes = listOf(ThemeMode.Evening, ThemeMode.Daylight)
    private val at = Instant.parse("2026-10-06T12:00:00Z")
    private val member = MemberRead(false, "self", "home", "Alex Smith", "user", "#FABD2F", "AS", "UTC", "manual", 1)
    private val row = ItemRead(id = "a", householdId = "home", title = "Laundry", notes = "Fold clothes", priority = 0,
        ownerId = "self", visibility = "household", kind = "todo", timeMode = "fixed", allDay = false,
        bufferBeforeMin = 0, bufferAfterMin = 0, version = 1, createdAt = at, updatedAt = at,
        assignees = listOf("self"), viewers = emptyList(), reminders = emptyList(), missedPolicy = "carry_over",
        capabilities = listOf("view", "complete", "change_status", "edit"), pendingDerivation = true,
        occurrence = OccurrenceRead("occ", "home", "a", 1, false, "open", "todo", "A", false, 1, at, at))
    @Composable private fun Frame(theme: ThemeMode, content: @Composable () -> Unit) {
        CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) { PorchlightTheme(theme, false, content) }
    }
    @Test fun todayBothThemesDoubleScaleChipsPendingAndCompleteTarget() {
        val theme = mutableStateOf(ThemeMode.Evening)
        var completed = false
        compose.setContent { Frame(theme.value) { TodayContent(listOf(TodayGroup(TodaySection.today, listOf(row))), 7, listOf(member), {}, { completed = true }) } }
        for (mode in themes) {
            compose.runOnIdle { theme.value = mode }
            compose.onNodeWithText("AS", useUnmergedTree = true).assertExists()
            compose.onNodeWithText("Pending derivation").assertExists()
            val action = compose.onNodeWithContentDescription("Complete Laundry").performScrollTo()
            val bounds = action.fetchSemanticsNode().boundsInRoot
            val density = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().targetContext.resources.displayMetrics.density
            assertTrue(bounds.height >= 48f * density && bounds.width >= 48f * density)
            action.performClick(); compose.runOnIdle { assertTrue(completed) }
        }
    }
    @Test fun boardDoubleScaleAlternativesBothThemes() {
        val theme = mutableStateOf(ThemeMode.Evening)
        var moved = false
        compose.setContent { Frame(theme.value) { BoardContent(boardColumns(listOf(row), null, "self"), listOf(member), {}, { _, _ -> moved = true }) } }
        for (mode in themes) {
            compose.runOnIdle { theme.value = mode }
            compose.onNodeWithContentDescription("Move Laundry to Doing").performScrollTo().performClick()
            compose.runOnIdle { assertTrue(moved) }
        }
    }
    @Test fun detailDoubleScaleHidesForbiddenActionsBothThemes() {
        val theme = mutableStateOf(ThemeMode.Evening)
        compose.setContent { Frame(theme.value) { ItemDetailContent(row.copy(capabilities = listOf("view")), listOf(member), {}, {}, {}, {}, {}) } }
        for (mode in themes) {
            compose.runOnIdle { theme.value = mode }
            compose.onNodeWithContentDescription("Edit Laundry").assertDoesNotExist()
            compose.onNodeWithContentDescription("Delete Laundry").assertDoesNotExist()
        }
    }
    @Test fun editorDoubleScaleValidationBothThemes() {
        val theme = mutableStateOf(ThemeMode.Evening)
        compose.setContent { Frame(theme.value) { EditorForm(EditorDraft(), {}, mapOf("title" to "Required"), listOf(member), false, EditorScope.All, {}) } }
        for (mode in themes) {
            compose.runOnIdle { theme.value = mode }
            compose.onNodeWithContentDescription("Title").assertIsDisplayed()
            compose.onNodeWithText("Required").assertExists()
            compose.onNodeWithContentDescription("Time zone").performScrollTo().assertIsDisplayed()
            compose.onNodeWithContentDescription("Title").performScrollTo()
        }
    }
}
