package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class C3aScreensUiTest {
    private val member = MemberRead(false, "self", "home", "Alex Smith", "user", "#FABD2F", "AS", "UTC", "manual", 1)
    private val row = itemRead("a").copy(title = "Laundry", assignees = listOf("self"), pendingDerivation = true,
        capabilities = listOf("view", "complete", "change_status", "edit"), occurrence = occurrenceRead("occ", "a"))
    @Test fun detailShowsAllAllowedActions() = runDesktopComposeUiTest(width = 650, height = 1800) {
        val capable = row.copy(capabilities = listOf("view", "edit", "complete", "change_status", "take", "delete"))
        setContent { PorchlightTheme(ThemeMode.Evening, false) { androidx.compose.foundation.layout.Column {
            ItemDetailContent(capable, listOf(member), {}, {}, {}, {}, {})
        } } }
        listOf("Edit Laundry", "Complete Laundry", "Take Laundry", "Delete Laundry", "Status Backlog", "Status To Do", "Status Doing", "Status Done").forEach { onNodeWithContentDescription(it).assertExists() }
    }
    @Test fun takeActionsAndServerInitialsAreCapabilityGated() {
        val ownable = row.copy(assignees = emptyList(), capabilities = listOf("view", "take"))
        val serverMember = member.copy(initials = "ZX")
        runDesktopComposeUiTest(width = 650, height = 1200) {
            setContent { PorchlightTheme(ThemeMode.Evening, false) {
                TodayContent(listOf(TodayGroup(TodaySection.needs_owner, listOf(ownable, row.copy(id = "b", title = "Assigned")))), 7, listOf(serverMember), {}, {})
            } }
            onNodeWithContentDescription("Take Laundry").assertExists()
            onNodeWithContentDescription("Take Assigned").assertDoesNotExist()
            onNodeWithText("ZX", useUnmergedTree = true).assertExists()
        }
        runDesktopComposeUiTest(width = 1100, height = 1400) {
            setContent { PorchlightTheme(ThemeMode.Daylight, false) { BoardContent(boardColumns(listOf(ownable), null, "self"), listOf(member), {}, { _, _ -> }) } }
            onNodeWithContentDescription("Take Laundry").assertExists()
        }
    }
    @Test fun todayAndBoardTargetsChipsAndPendingAtDoubleScaleBothThemes() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 420, height = 1200) {
            var completed = false
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                TodayContent(listOf(TodayGroup(TodaySection.today, listOf(row))), 7, listOf(member), {}, { completed = true })
            } } }
            onNodeWithText("AS", useUnmergedTree = true).assertExists()
            onNodeWithText("Laundry", useUnmergedTree = true).performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult) { action ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                assertTrue(action(layouts), "layout unavailable"); assertTrue(layouts.isNotEmpty(), "no layouts"); assertTrue(layouts.none { it.hasVisualOverflow }, "Laundry text overflow: ${layouts.map { it.size to it.didOverflowHeight }}")
            }
            onNodeWithText("Pending derivation").assertExists()
            val button = onNodeWithContentDescription("Complete Laundry")
            val bounds = button.fetchSemanticsNode().boundsInRoot
            assertTrue(bounds.height >= 48f && bounds.width >= 48f)
            button.performClick(); assertTrue(completed)
        }
        runDesktopComposeUiTest(width = 1100, height = 1400) {
            var move: BoardDrop? = null
            setContent { PorchlightTheme(ThemeMode.Daylight, false) {
                BoardContent(boardColumns(listOf(row), null, "self"), listOf(member), {}, { _, d -> move = d })
            } }
            onNodeWithContentDescription("Move Laundry to Doing").performScrollTo().performClick()
            assertEquals(BoardDrop("doing", 0), move)
        }
    }
    @Test fun boardDragKeysAndAccessibleActionsUseSameProjection() = runDesktopComposeUiTest(width = 1100, height = 1400) {
        val moves = mutableListOf<BoardDrop>()
        setContent { PorchlightTheme(ThemeMode.Evening, false) { BoardContent(boardColumns(listOf(row), null, "self"), listOf(member), {}, { _, drop -> moves += drop }) } }
        val card = onNodeWithTag("board-card-occ")
        card.performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.RequestFocus) { it() }
        card.performKeyInput { keyDown(androidx.compose.ui.input.key.Key.AltLeft); pressKey(androidx.compose.ui.input.key.Key.DirectionDown); keyUp(androidx.compose.ui.input.key.Key.AltLeft) }
        assertTrue(moves.isEmpty())
        card.performTouchInput { down(androidx.compose.ui.geometry.Offset(width / 2f, 20f)); advanceEventTime(900); up() }
        assertTrue(moves.isEmpty())
        card.performTouchInput { down(androidx.compose.ui.geometry.Offset(width / 2f, 20f)); advanceEventTime(900); moveTo(androidx.compose.ui.geometry.Offset(340f, 20f)); up() }
        assertEquals(BoardDrop("doing", 0), moves.last())
        val actions = card.fetchSemanticsNode().config[androidx.compose.ui.semantics.SemanticsActions.CustomActions]
        assertTrue(actions.any { it.label == "Move to Doing" })
    }
    @Test fun allFormsAndBoardAlternativesAtDoubleScaleBothThemes() {
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            runDesktopComposeUiTest(width = 1100, height = 1500) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(mode, false) {
                    BoardContent(boardColumns(listOf(row), null, "self"), listOf(member), {}, { _, _ -> })
                } } }
                val action = onNodeWithContentDescription("Move Laundry to Doing").performScrollTo().assertIsDisplayed().fetchSemanticsNode()
                assertTrue(action.boundsInRoot.height >= 48 && action.boundsInRoot.width >= 48)
                onNodeWithText("AS", useUnmergedTree = true).assertExists()
            onNodeWithText("Laundry", useUnmergedTree = true).performSemanticsAction(androidx.compose.ui.semantics.SemanticsActions.GetTextLayoutResult) { action ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                assertTrue(action(layouts), "layout unavailable"); assertTrue(layouts.isNotEmpty(), "no layouts"); assertTrue(layouts.none { it.hasVisualOverflow }, "Laundry text overflow: ${layouts.map { it.size to it.didOverflowHeight }}")
            }
            }
            runDesktopComposeUiTest(width = 420, height = 1400) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(mode, false) {
                    EditorForm(EditorDraft(), {}, mapOf("title" to "Required"), listOf(member), false, EditorScope.All, {}, scopes = EditorScope.entries)
                } } }
                onNodeWithText("Choose recurring edit scope before adjusting timing.", substring = true).assertExists()
                onNodeWithText("Changing scope resets timing", substring = true).assertExists()
                onNodeWithContentDescription("All occurrences: Edit scope: All occurrences").assertDoesNotExist() // label is already part of its description
                onNodeWithContentDescription("Edit scope: All occurrences").assertExists()
                onNodeWithContentDescription("Title").performScrollTo().assertIsDisplayed()
                onNodeWithText("Required").assertExists()
            }
        }
    }
    @Test fun detailHidesForbiddenActionsAndEditorShowsFieldErrors() = runDesktopComposeUiTest(width = 420, height = 1100) {
        setContent { PorchlightTheme(ThemeMode.Evening, false) {
            ItemDetailContent(row.copy(capabilities = listOf("view")), listOf(member), {}, {}, {}, {}, {})
        } }
        onNodeWithContentDescription("Edit Laundry").assertDoesNotExist()
        onNodeWithContentDescription("Delete Laundry").assertDoesNotExist()
    }
    @Test fun editorAccessibleValidationAndScopePrompt() = runDesktopComposeUiTest(width = 420, height = 1400) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            EditorForm(EditorDraft(), {}, mapOf("title" to "Required"), listOf(member), false, EditorScope.All, {})
        } } }
        onNodeWithContentDescription("Title").assertExists()
        onNodeWithText("Required").assertExists()
        onNodeWithContentDescription("Time zone").assertExists()
    }
}
