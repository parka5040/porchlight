package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.layout.*
import androidx.compose.material3.Text
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.*
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class BoardMeasurementRegressionTest {
    @Test fun ni6RowUsesRealChildHeightEvenWhenIntrinsicHeightUnderestimates() = runDesktopComposeUiTest(width = 800, height = 600) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight) {
            BoardCells(boardStatuses.map { BoardColumn(it, emptyList()) }) { column, modifier ->
                Layout(modifier = modifier, content = {
                    Column {
                        repeat(16) { Text("Long measured content that wraps with the available cell width") }
                    }
                }, measurePolicy = object : MeasurePolicy {
                    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
                        val child = measurables.single().measure(constraints)
                        return layout(child.width, child.height) { child.place(0, 0) }
                    }
                    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = 0
                })
            }
        } } }
        boardStatuses.forEach { status ->
            val content = onNodeWithTag("board-content-$status").getUnclippedBoundsInRoot()
            val allocation = onNodeWithTag("board-cell-$status").getUnclippedBoundsInRoot()
            val texts = onAllNodes(hasAnyAncestor(hasTestTag("board-content-$status")) and hasText("Long measured content that wraps with the available cell width"), useUnmergedTree = true)
            assertEquals(16, texts.fetchSemanticsNodes().size)
            for (index in 0 until 16) {
                val results = mutableListOf<TextLayoutResult>()
                texts[index].performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(results) }
                assertTrue(results.all { !it.didOverflowHeight }, "Allocation cannot truncate a child whose intrinsic height under-reports")
                assertTrue(texts[index].getUnclippedBoundsInRoot().bottom <= allocation.bottom)
            }
            assertTrue(allocation.height >= content.height)
        }
    }
    @Test fun m6EmptySlotStillPairsEveryColumnWithItsOwnAllocation() = variedRoots(emptySlot = true)
    @Test fun m6SeveralRootsStayWithinTheirOwnColumnAllocation() = variedRoots(emptySlot = false)
    private fun variedRoots(emptySlot: Boolean) = runDesktopComposeUiTest(width = 800, height = 600) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight) {
            BoardCells(boardStatuses.map { BoardColumn(it, emptyList()) }) { column, _ ->
                val count = if (column.status == boardStatuses.first()) { if (emptySlot) 0 else 3 } else 1
                repeat(count) { index -> Text("Measured ${column.status} root $index with wrapping content",
                    Modifier.testTag("root-${column.status}-$index")) }
            }
        } } }
        boardStatuses.forEach { status ->
            val allocation = onNodeWithTag("board-cell-$status").getUnclippedBoundsInRoot()
            val count = if (status == boardStatuses.first()) { if (emptySlot) 0 else 3 } else 1
            var bottom = allocation.top
            repeat(count) { index ->
                val root = onNodeWithTag("root-$status-$index").getUnclippedBoundsInRoot()
                assertTrue(root.left >= allocation.left && root.right <= allocation.right, "Root belongs to $status, not its neighbour")
                assertTrue(root.top >= bottom && root.bottom <= allocation.bottom, "Roots are stacked, not overlapped or clipped")
                bottom = root.bottom
            }
            assertTrue(allocation.height.value > 0, "Even an empty slot retains a usable drop allocation")
        }
    }
    @Test fun ni4PartlyClippedCardUsesItsUnclippedMidpointForInsertion() = runDesktopComposeUiTest(width = 800, height = 600) {
        val target = itemRead("target").copy(title = "Tall target", capabilities = listOf("view", "change_status"), occurrence = occurrenceRead("target-occ", "target").copy(status = "todo"))
        val source = itemRead("source").copy(title = "Moving card", capabilities = listOf("view", "change_status"), occurrence = occurrenceRead("source-occ", "source").copy(status = "backlog"))
        val more = List(5) { i -> itemRead("extra-$i").copy(occurrence = occurrenceRead("extra-occ-$i", "extra-$i", "B$i").copy(status = "todo")) }
        var result: BoardDrop? = null
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f)) { PorchlightTheme(ThemeMode.Daylight) {
            BoardContent(boardColumns(listOf(source, target) + more, null, "self"), emptyList(), {}, { _, drop -> result = drop })
        } } }
        val scroller = onNodeWithTag("board-columns")
        val targetCard = onNodeWithTag("board-card-target-occ")
        val original = targetCard.getUnclippedBoundsInRoot()
        val viewport = scroller.fetchSemanticsNode().boundsInRoot
        scroller.performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, original.top.value - viewport.top + original.height.value * 0.35f) }
        val full = targetCard.getUnclippedBoundsInRoot()
        val clipped = targetCard.fetchSemanticsNode().boundsInRoot
        assertTrue(full.top.value < clipped.top && full.bottom.value > clipped.top, "Partly clipped target: full=$full clipped=$clipped viewport=$viewport offset=${scroller.fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()}")
        // Between the true midpoint and the midpoint of the clipped rect: AFTER target, not before it.
        val release = Offset(clipped.center.x, ((full.top.value + full.bottom.value) / 2 + clipped.center.y) / 2)
        assertTrue(release.y > (full.top.value + full.bottom.value) / 2 && release.y < clipped.center.y)
        val sourceCard = onNodeWithTag("board-card-source-occ")
        val visible = sourceCard.fetchSemanticsNode().boundsInRoot
        assertTrue(visible.height > 0)
        sourceCard.performTouchInput {
            down(Offset(visible.width / 2, minOf(visible.height / 2, PorchlightTokens.spaceInset.toFloat())))
            advanceEventTime(900)
            moveTo(release - visible.topLeft)
            up()
        }
        waitForIdle()
        assertEquals(BoardDrop("todo", 1), result, "Scrolled target midpoint determines insertion")
    }

    @Test fun ni6LongTitlesDoubleFontAndManyMemberChipsNeverClipCellContent() {
        for (width in listOf(360, 800, 1024, 1440)) runDesktopComposeUiTest(width = width, height = 800) {
            val members = List(12) { i -> MemberRead(false, "member-$i", "home", "Household member number $i", "user", "#FABD2F", "M$i", "UTC", "manual", 1) }
            val rows = List(4) { index -> itemRead("long-$index").copy(title = "Prepare the complete household supplies checklist and review every seasonal maintenance task before the shared planning session $index",
                assignees = members.map { it.id }, capabilities = listOf("view", "take", "change_status"),
                occurrence = occurrenceRead("long-occ-$index", "long-$index").copy(status = boardStatuses[index])) }
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight) {
                BoardContent(boardColumns(rows, null, "self"), members, {}, { _, _ -> })
            } } }
            rows.forEach { row ->
                val cell = onNodeWithTag("board-cell-${row.occurrence!!.status}").getUnclippedBoundsInRoot()
                val cardTag = "board-card-${row.occurrence!!.id}"
                val card = onNodeWithTag(cardTag).getUnclippedBoundsInRoot()
                assertTrue(card.bottom <= cell.bottom, "Measured card fits allocated cell")
                val textNodes = onAllNodes(hasAnyAncestor(hasTestTag(cardTag)) and SemanticsMatcher.keyIsDefined(SemanticsActions.GetTextLayoutResult), useUnmergedTree = true)
                textNodes.fetchSemanticsNodes().forEachIndexed { index, _ ->
                    val layouts = mutableListOf<TextLayoutResult>()
                    textNodes[index].performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                    assertTrue(layouts.isNotEmpty())
                    layouts.forEach { result ->
                        assertFalse(result.didOverflowHeight, "No clipped text height at $width: ${result.layoutInput.text}")
                        // Text's semantics result can retain the full paragraph constraint while the
                        // node shrink-wraps its ink. Check actual line extents, not didOverflowWidth.
                        for (line in 0 until result.lineCount) assertTrue(result.getLineRight(line) <= result.size.width + 1f,
                            "No clipped line at $width: ${result.layoutInput.text}")
                    }
                    val text = textNodes[index].getUnclippedBoundsInRoot()
                    assertTrue(text.top >= card.top && text.bottom <= card.bottom, "Every chip, title and action stays inside card")
                }
                val lastAction = boardStatuses.last { it != row.occurrence!!.status }
                onNodeWithContentDescription("Move ${row.title} to ${statusLabel(lastAction)}").performScrollTo().assertIsDisplayed()
            }
        }
    }
}
