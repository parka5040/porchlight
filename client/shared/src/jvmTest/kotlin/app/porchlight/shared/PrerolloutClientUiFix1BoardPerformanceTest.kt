package app.porchlight.shared

import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.board.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import java.io.File
import kotlin.system.measureTimeMillis
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiFix1BoardPerformanceTest {
    @Test fun measure500Cards() = measure(500)
    @Test fun measure2000Cards() = measure(2_000)
    private fun measure(count: Int) = runDesktopComposeUiTest(width = 1024, height = 800) {
        val rows = List(count) { i -> itemRead("row-$i").copy(title = "Household task $i", capabilities = listOf("view"),
            occurrence = occurrenceRead("occ-$i", "row-$i").copy(status = "todo")) }
        val elapsed = measureTimeMillis {
            setContent { PorchlightTheme(ThemeMode.Daylight) {
                BoardContent(boardStatuses.map { BoardColumn(it, if (it == "todo") rows else emptyList()) }, emptyList(), {}, { _, _ -> })
            } }
            waitForIdle()
        }
        val cards = onAllNodes(hasContentDescription("Board card", substring = true)).fetchSemanticsNodes().size
        val nodes = onAllNodes(SemanticsMatcher("all nodes") { true }, useUnmergedTree = true).fetchSemanticsNodes().size
        assertEquals(count, cards)
        val report = "Eager Board, 1024x800, font 1x, view-only cards=$count: initial composition/layout/idle=${elapsed}ms; card nodes=$cards; unmerged semantics nodes=$nodes\n"
        File("build/test-evidence/prerollout-client-ui-fix1").apply { mkdirs() }.resolve("board-$count-performance.txt").writeText(report)
        println(report)
    }
}
