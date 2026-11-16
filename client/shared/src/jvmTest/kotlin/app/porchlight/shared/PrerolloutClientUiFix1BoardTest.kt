package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.runtime.saveable.*
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusManager
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiFix1BoardTest {
    private fun row(i: Int, status: String = "todo", movable: Boolean = false): ItemRead = itemRead("row-$i").copy(title = "Row $i",
        capabilities = if (movable) listOf("view", "change_status") else listOf("view"),
        occurrence = occurrenceRead("occ-$i", "row-$i", i.toString().padStart(4, '0')).copy(status = status))

    @Test fun dropBelowAllDestinationCardsAppends() = runDesktopComposeUiTest(width = 1024, height = 600) {
        val source = List(10) { row(it, "backlog", true) }
        var result: BoardDrop? = null
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("backlog", source), BoardColumn("todo", listOf(row(100, movable = true)))), emptyList(), {}, { _, drop -> result = drop })
        } }
        val scroll = onNodeWithTag("board-columns")
        scroll.performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 900f) }
        val sourceCard = onAllNodes(hasContentDescription("Board card Row", substring = true)).fetchSemanticsNodes().first { it.boundsInRoot.height > 100 && it.boundsInRoot.top > 0 }
        val sourceTag = sourceCard.config[SemanticsProperties.TestTag]
        val bounds = onNodeWithTag(sourceTag).fetchSemanticsNode().boundsInRoot
        val target = onNodeWithTag("board-cell-todo").fetchSemanticsNode().boundsInRoot
        assertTrue(onNodeWithTag("board-card-occ-100").getUnclippedBoundsInRoot().bottom.value < target.top)
        onNodeWithTag(sourceTag).performTouchInput {
            down(Offset(bounds.width / 2, 20f)); advanceEventTime(900)
            moveTo(Offset(target.center.x, target.top + 100f) - bounds.topLeft); up()
        }
        assertEquals(BoardDrop("todo", 1), result)
    }

    @Test fun nextFocusVisitsEveryCardBeyondViewportAndBack() = traversal(FocusDirection.Next, FocusDirection.Previous)
    @Test fun directionalFocusVisitsEveryCardBeyondViewportAndBack() = traversal(FocusDirection.Down, FocusDirection.Up)
    private fun traversal(forward: FocusDirection, backward: FocusDirection) = runDesktopComposeUiTest(width = 320, height = 600) {
        lateinit var focus: FocusManager
        setContent { focus = LocalFocusManager.current; PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("todo", List(18) { row(it) })), emptyList(), {}, { _, _ -> })
        } }
        onNodeWithTag("board-card-occ-0").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        fun visit(i: Int, direction: FocusDirection) {
            val tag = "board-card-occ-$i"
            val matcher = isFocused() and (hasTestTag(tag) or hasAnyAncestor(hasTestTag(tag)))
            repeat(4) {
                if (onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()) return
                onRoot().performKeyInput {
                    when (direction) {
                        FocusDirection.Next -> pressKey(Key.Tab)
                        FocusDirection.Previous -> { keyDown(Key.ShiftLeft); pressKey(Key.Tab); keyUp(Key.ShiftLeft) }
                        FocusDirection.Down -> pressKey(Key.DirectionDown)
                        else -> pressKey(Key.DirectionUp)
                    }
                }
            }
            assertTrue(onAllNodes(matcher, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty(), "Focus must reach occurrence $i using $direction")
        }
        for (i in 1..17) visit(i, forward)
        for (i in 16 downTo 0) visit(i, backward)
    }

    @Test fun multiColumnNextFocusVisitsWholeColumnBeforeNextColumn() = runDesktopComposeUiTest(width = 1024, height = 600) {
        lateinit var focus: FocusManager
        setContent { focus = LocalFocusManager.current; PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("backlog", List(18) { row(it, "backlog") }), BoardColumn("todo", List(4) { row(it + 100) })), emptyList(), {}, { _, _ -> })
        } }
        onNodeWithTag("board-card-occ-0").performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        val visited = mutableListOf<Int>()
        repeat(60) {
            for (i in (0..17).toList() + (100..103).toList()) {
                if (onAllNodes(isFocused() and hasTestTag("board-card-occ-$i")).fetchSemanticsNodes().isNotEmpty() && visited.lastOrNull() != i) visited += i
            }
            runOnIdle { focus.moveFocus(FocusDirection.Next) }
        }
        assertEquals((0..17).toList() + (100..103).toList(), visited.take(22), "Traversal follows complete occurrence order, not currently composed cards")
    }

    @Test fun tabShiftTabAndAltMovesKeepTheFocusedOccurrence() = runDesktopComposeUiTest(width = 320, height = 600) {
        val rows = mutableStateOf(List(24) { row(it, movable = true) })
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("todo", rows.value)), emptyList(), {}, { moving, drop ->
                rows.value = rows.value.filter { it.id != moving.id }.toMutableList().also { it.add(drop.index, moving) }
            })
        } }
        val tag = "board-card-occ-1"
        onNodeWithTag(tag).performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        repeat(18) {
            onRoot().performKeyInput { keyDown(Key.AltLeft); pressKey(Key.DirectionDown); keyUp(Key.AltLeft) }
            onNodeWithTag(tag).assertIsFocused()
        }
        onNodeWithTag(tag).assertIsDisplayed()
        onRoot().performKeyInput { pressKey(Key.Tab) }
        assertTrue(onAllNodes(isFocused() and hasAnyAncestor(hasTestTag(tag)), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty())
        onRoot().performKeyInput { keyDown(Key.ShiftLeft); pressKey(Key.Tab); keyUp(Key.ShiftLeft) }
        onNodeWithTag(tag).assertIsFocused()
    }

    @Test fun altMoveAcrossColumnsRetainsOccurrenceFocus() = runDesktopComposeUiTest(width = 1024, height = 600) {
        val columns = mutableStateOf(listOf(BoardColumn("backlog", List(12) { row(it, "backlog", true) }), BoardColumn("todo", List(12) { row(it + 100, movable = true) })))
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(columns.value, emptyList(), {}, { moving, drop ->
                columns.value = columns.value.map { column ->
                    val cards = column.cards.filter { it.id != moving.id }.toMutableList()
                    if (column.status == drop.status) cards.add(drop.index, moving.copy(occurrence = moving.occurrence!!.copy(status = drop.status)))
                    column.copy(cards = cards)
                }
            })
        } }
        val card = onNodeWithTag("board-card-occ-8")
        card.performSemanticsAction(SemanticsActions.RequestFocus) { it() }
        for (key in listOf(Key.DirectionRight, Key.DirectionLeft)) {
            onRoot().performKeyInput { keyDown(Key.AltLeft); pressKey(key); keyUp(Key.AltLeft) }
            card.assertIsFocused().assertIsDisplayed()
        }
    }

    @Test fun semanticsIncludeEveryOccurrenceInColumnOrder() = runDesktopComposeUiTest(width = 1024, height = 600) {
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("backlog", List(24) { row(it, "backlog") }), BoardColumn("todo", List(8) { row(it + 100) })), emptyList(), {}, { _, _ -> })
        } }
        val nodes = onAllNodes(hasContentDescription("Board card", substring = true)).fetchSemanticsNodes()
        val actual = nodes.map { it.config[SemanticsProperties.ContentDescription].single() }
        assertEquals(((0..23).toList() + (100..107).toList()).map { "Board card Row $it" }, actual)
        val traversal = nodes.map { assertNotNull(it.config.getOrNull(SemanticsProperties.TraversalIndex), "Each card has an explicit accessibility traversal index") }
        assertEquals(traversal.sorted(), traversal)
        assertEquals(traversal.size, traversal.distinct().size)
        assertTrue(onNodeWithTag("board-columns").fetchSemanticsNode().config[SemanticsProperties.IsTraversalGroup])
    }

    @Test fun visibleOccurrenceSurvivesUnrelatedHeightChangeAndSavedRecreation() = runDesktopComposeUiTest(width = 1024, height = 600) {
        val paneWidth = mutableStateOf(320)
        val rows = mutableStateOf(List(30) { i -> row(i).copy(title = if (i % 2 == 0) "Row $i" else "Long wrapping household task title ".repeat(5) + i) })
        val mounted = mutableStateOf(true)
        var registry = SaveableStateRegistry(null) { true }
        setContent { if (mounted.value) CompositionLocalProvider(LocalSaveableStateRegistry provides registry) { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(listOf(BoardColumn("todo", rows.value)), emptyList(), {}, { _, _ -> }, Modifier.width(paneWidth.value.dp).fillMaxHeight())
        } } }
        val scroll = onNodeWithTag("board-columns")
        repeat(10) { scroll.performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 300f) } }
        fun anchor(): Pair<String, Float> {
            val top = scroll.fetchSemanticsNode().boundsInRoot.top
            val first = onAllNodes(hasContentDescription("Board card", substring = true)).fetchSemanticsNodes()
                .filter { it.boundsInRoot.bottom > top && it.boundsInRoot.height > 0 }.minBy { it.boundsInRoot.top }
            val tag = first.config[SemanticsProperties.TestTag]
            return tag to onNodeWithTag(tag).getUnclippedBoundsInRoot().top.value
        }
        val before = anchor()
        runOnIdle { rows.value = rows.value.mapIndexed { i, item -> if (i == 0) item.copy(title = "Changed unrelated task ".repeat(20)) else item } }
        waitForIdle()
        assertEquals(before.first, anchor().first, "Unrelated height change preserves visible occurrence")
        assertEquals(before.second, anchor().second, 1f, "Unrelated height change preserves local offset")
        val saved = runOnIdle { registry.performSave().also { mounted.value = false } }
        waitForIdle()
        runOnIdle { registry = SaveableStateRegistry(saved) { true }; mounted.value = true }
        waitForIdle()
        assertEquals(before.first, anchor().first, "Saved recreation preserves visible occurrence")
        assertEquals(before.second, anchor().second, 1f)
        for (width in listOf(560, 320)) {
            runOnIdle { paneWidth.value = width }
            waitForIdle()
            assertEquals(before.first, anchor().first, "Pane reflow preserves visible occurrence")
            assertEquals(before.second, anchor().second, 1f, "Pane reflow preserves local offset")
        }
    }
}
