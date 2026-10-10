package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
import kotlin.math.abs
import kotlin.test.*
import kotlin.time.Instant

/** Density-one dimensions are harness sizes, never production layout breakpoints. */
@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class BoardGridUiTest(private val width: Int, private val platform: PlatformKind,
    private val scale: Float, private val theme: ThemeMode) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{0}dp-{1}-{2}x-{3}")
        fun cases(): List<Array<Any>> = (listOf(800, 960, 1024, 1440, 1600, 1920).map { it to PlatformKind.Desktop } +
            listOf(320, 360, 600, 840).map { it to PlatformKind.Android }).flatMap { (width, platform) ->
            listOf(1f, 2f).flatMap { scale -> listOf(ThemeMode.Daylight, ThemeMode.Evening).map { arrayOf(width, platform, scale, it) } }
        }
        private const val longTitle = "Prepare the household supplies and review the seasonal checklist before the next shared planning session"
        private val statuses = listOf("backlog", "todo", "doing", "done")
    }
    private fun screen(overflow: Boolean = false, height: Int = 1100, title: String = longTitle, longStatus: String = "doing",
        test: ComposeUiTest.(HouseholdData) -> Unit) {
        val now = Instant.parse("2026-10-09T12:00:00Z")
        val cache = memoryCache { now }
        var rows = listOf(
            itemRead("card").copy(title = "Fixture card", capabilities = listOf("view", "change_status"),
                occurrence = occurrenceRead("occ-card", "card").copy(status = "backlog")),
            itemRead("long").copy(title = title, capabilities = listOf("view"),
                occurrence = occurrenceRead("occ-long", "long").copy(status = longStatus)),
        )
        if (overflow) rows += List(4) { index ->
            itemRead("extra-$index").copy(title = "Additional fixture $index", capabilities = listOf("view"),
                occurrence = occurrenceRead("occ-extra-$index", "extra-$index", "B$index").copy(status = "backlog"))
        }
        val snapshot = emptySnapshot(1).copy(memberSettings = SettingsRead("self", 1,
            TodaySettings(listOf(TodaySection.today), SettingsScope.household, 7)))
        runBlocking { cache.install(snapshot) }
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            val body = when {
                request.method == HttpMethod.Patch && request.url.encodedPath == "/v1/occurrences/occ-card" -> {
                    val status = WireJson.parseToJsonElement(request.body.toByteArray().decodeToString()).jsonObject.getValue("status").jsonPrimitive.content
                    rows = rows.map { if (it.id == "card") it.copy(occurrence = it.occurrence!!.copy(status = status, version = 2)) else it }
                    WireJson.encodeToString(OccurrenceResponse.serializer(), occurrenceResponse("occ-card", "card").copy(status = OccurrenceStatus.entries.first { it.value == status }, version = 2))
                }
                request.url.encodedPath == "/v1/items/card" -> WireJson.encodeToString(ItemResponse.serializer(), itemResponse("card").copy(title = "Fixture card"))
                request.url.encodedPath == "/v1/items/card/occurrences" -> """{"items":[${WireJson.encodeToString(OccurrenceResponse.serializer(), occurrenceResponse("occ-card", "card").copy(status = OccurrenceStatus.entries.first { it.value == rows.first().occurrence!!.status }, version = rows.first().occurrence!!.version))}],"next_cursor":null}"""
                request.url.encodedPath == "/v1/board" -> """{"items":[${rows.joinToString(",") { WireJson.encodeToString(ItemRead.serializer(), it) }}],"next_cursor":null}"""
                request.url.encodedPath == "/v1/snapshot" -> WireJson.encodeToString(SnapshotRead.serializer(), snapshot)
                else -> """{"items":[],"next_cursor":null}"""
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = ScreenSession()
            override val clock = AppClock { now }
            override val platform = object : PlatformInfo {
                override val kind: PlatformKind = this@BoardGridUiTest.platform
                override val systemDark = MutableStateFlow(theme == ThemeMode.Evening)
            }
        }
        try {
            runDesktopComposeUiTest(width = width, height = height) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale), LocalScreenServices provides services) {
                    PorchlightTheme(theme) { Surface(Modifier.fillMaxSize()) {
                        ShellNavigation(AppRoute.Board(), DeviceSettings(), services.settings, {}, services = services)
                    } }
                } }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithTag("board-card-occ-card").fetchSemanticsNodes().isNotEmpty() }
                test(data)
            }
        } finally { services.screenSession.writeScope.cancel(); data.close(); api.close(); cache.close() }
    }
    private fun close(a: Float, b: Float, message: String) = assertTrue(abs(a - b) <= 1f, "$message: $a / $b")
    private fun textLayout(node: SemanticsNodeInteraction): TextLayoutResult {
        val results = mutableListOf<TextLayoutResult>()
        node.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(results) }
        return results.single()
    }
    private fun ComposeUiTest.across(): Int {
        val cells = statuses.map { onNodeWithTag("board-cell-$it").getUnclippedBoundsInRoot() }
        return cells.count { abs(it.top.value - cells.first().top.value) <= 1f }
    }
    @Test fun fix1R1UnselectedPaneUsesAllAvailableWidth() = screen {
        onNodeWithTag("detail-pane").assertDoesNotExist()
        assertEquals(when (width) { 320, 360 -> 1; 1440, 1600, 1920 -> 4; else -> 2 }, across())
    }
    @Test fun fix1R1SelectedItemReflowsBoardAndBackRestoresIt() {
        assumeTrue(width >= 600 && theme == ThemeMode.Daylight)
        screen {
            onNodeWithContentDescription("Open Fixture card").performScrollTo().performClick()
            if (width >= PorchlightTokens.expandedBreakpoint) {
                onNodeWithTag("detail-pane").assertIsDisplayed()
                onNodeWithText("DESIGN-PENDING: Detail pane").assertIsDisplayed()
                assertEquals(when (width) { 840, 960 -> 1; 1920 -> 4; else -> 2 }, across())
            } else {
                onNodeWithTag("detail-pane").assertDoesNotExist()
                onNodeWithTag("board-columns").assertDoesNotExist()
            }
            onNodeWithContentDescription("Refetch item").assertIsDisplayed()
            onNodeWithContentDescription("Go back").performClick()
            onNodeWithTag("detail-pane").assertDoesNotExist()
            assertEquals(if (width >= 1440) 4 else 2, across())
        }
    }
    @Test fun fix1R2OneScrollWithContentHeightRowsAtRealisticHeights() {
        assumeTrue(width in listOf(360, 800, 1024))
        val heights = when (width) { 360 -> listOf(640, 800); 800 -> listOf(600); else -> listOf(768) }
        heights.forEach { height -> screen(height = height) {
            val scrolls = onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.VerticalScrollAxisRange) and
                (hasTestTag("board-columns") or hasAnyAncestor(hasTestTag("board-columns"))), useUnmergedTree = true).fetchSemanticsNodes()
            assertEquals(1, scrolls.size, "Board must have exactly one vertical scroll")
            statuses.forEach { status ->
                val cell = onNodeWithTag("board-cell-$status").getUnclippedBoundsInRoot()
                onAllNodes(hasTestTag("board-card-occ-card") or hasTestTag("board-card-occ-long"))
                    .fetchSemanticsNodes().filter { node -> generateSequence(node.parent) { it.parent }.any { it.config.getOrNull(SemanticsProperties.TestTag) == "board-content-$status" } }
                    .forEach { node ->
                        val card = onNodeWithTag(node.config[SemanticsProperties.TestTag]).getUnclippedBoundsInRoot()
                        assertTrue(card.top >= cell.top && card.bottom <= cell.bottom, "Content is never confined to a sub-viewport")
                    }
            }
            if (width == 800 && scale == 1f) {
                // First row's populated cell shows its title AND entire first action, without a cell scroll.
                onNodeWithText("Fixture card", useUnmergedTree = true).assertIsDisplayed()
                val action = onNodeWithContentDescription("Open Fixture card")
                val full = action.getUnclippedBoundsInRoot()
                val visible = action.fetchSemanticsNode().boundsInRoot
                close(full.height.value, visible.height, "First action visible at 800x600")
            }
        } }
    }
    @Test fun fix1R2BothPopulatedFirstRowCellsShowTitleAndFirstAction() {
        assumeTrue(width == 800 && scale == 1f)
        screen(height = 600, title = "Other fixture", longStatus = "todo") {
            listOf("Fixture card", "Other fixture").forEach { title ->
                onNodeWithText(title, useUnmergedTree = true).assertIsDisplayed()
                val action = onNodeWithContentDescription("Open $title").assertIsDisplayed()
                close(action.getUnclippedBoundsInRoot().height.value, action.fetchSemanticsNode().boundsInRoot.height,
                    "Each populated cell shows its whole first action on first render")
            }
        }
    }
    @Test fun fix1R2LastRowExtendsToViewportAndNeverShortensTallContent() = screen {
        val viewport = onNodeWithTag("board-columns").fetchSemanticsNode().boundsInRoot
        val lastCell = onNodeWithTag("board-cell-done").getUnclippedBoundsInRoot()
        assertTrue(lastCell.bottom.value >= viewport.bottom, "Last row supplies an empty lower drop target")
        val first = onNodeWithTag("board-cell-backlog").getUnclippedBoundsInRoot()
        val card = onNodeWithTag("board-card-occ-card").getUnclippedBoundsInRoot()
        assertTrue(first.bottom >= card.bottom, "Tall content sets the shared row height")
    }
    @Test fun fix1R4UnbreakableCardTokenDoesNotChangeArrangement() {
        assumeTrue(width in listOf(800, 1024))
        val token = "f".repeat(44) + "@example.invalid"
        assertEquals(60, token.length)
        var expected = 0
        screen { expected = across() }
        screen(title = token) {
            assertEquals(expected, across(), "Card content cannot choose the arrangement")
            val layout = textLayout(onNodeWithText(token, useUnmergedTree = true))
            assertTrue(layout.lineCount > 1)
            assertFalse(layout.hasVisualOverflow)
        }
    }
    @Test fun longTitleCannotSetUnboundedColumnWidths() = screen {
        val card = onNodeWithTag("board-card-occ-long").getUnclippedBoundsInRoot()
        assertTrue(card.width <= PorchlightTokens.detailWidth.dp, "Long title widened column to ${card.width}")
        assertTrue(textLayout(onNodeWithText(longTitle, useUnmergedTree = true)).lineCount > 1, "Long title must wrap")
    }
    @Test fun allHeadersStayHorizontallyInsideTheRealPane() = screen {
        val pane = onNodeWithTag("shell-content").getUnclippedBoundsInRoot()
        val right = pane.right
        statuses.forEach { status ->
            val heading = onNodeWithTag("board-heading-$status").getUnclippedBoundsInRoot()
            assertTrue(heading.left >= pane.left && heading.right <= right,
                "$status header outside pane ${pane.left}..$right: $heading")
        }
    }
    @Test fun arrangementCellsHeadersAndLongTitle() = screen {
        onNodeWithTag("detail-pane").assertDoesNotExist()
        val viewport = onNodeWithTag("board-columns").getUnclippedBoundsInRoot()
        onNodeWithTag("board-columns").assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.HorizontalScrollAxisRange))
        val cells = statuses.map { onNodeWithTag("board-cell-$it").getUnclippedBoundsInRoot() }
        val across = cells.count { abs(it.top.value - cells.first().top.value) <= 1f }
        assertTrue(across in listOf(1, 2, 4), "Only 4-up, 2x2 or stacked: $cells")
        // Policy examples in the real shell with no selected item (rail, no empty detail pane).
        assertEquals(when (width) { 320, 360 -> 1; 1440, 1600, 1920 -> 4; else -> 2 }, across,
            "Fixture arrangement at ${width}dp / ${scale}x")
        if (width <= 360) assertEquals(1, across)
        cells.chunked(across).forEach { row ->
            close(viewport.left.value, row.first().left.value, "Each row starts at content edge")
            row.forEach { cell ->
                close(row.first().top.value, cell.top.value, "Shared row top")
                close(row.first().height.value, cell.height.value, "Generous equal-height drop cells")
                close(row.first().width.value, cell.width.value, "Equal-width cells")
                assertTrue(cell.left >= viewport.left && cell.right <= viewport.right, "No partial column")
                assertTrue(cell.width <= PorchlightTokens.detailWidth.dp, "Readable width cap")
            }
            row.zipWithNext().forEach { (a, b) -> close(PorchlightTokens.spaceMd.toFloat(), b.left.value - a.right.value, "Token column gap") }
        }
        statuses.forEachIndexed { index, status ->
            onNodeWithTag("board-cell-$status").performScrollTo()
            val heading = onNodeWithTag("board-heading-$status")
            heading.performScrollTo().assertIsDisplayed()
            heading.assertTextEquals(listOf("Backlog (1)", "To Do (0)", "Doing (1)", "Done (0)")[index])
            val layout = textLayout(heading)
            assertFalse(layout.hasVisualOverflow, "Header overflow: $status ${layout.size}, w=${layout.didOverflowWidth}, h=${layout.didOverflowHeight}, natural=${layout.multiParagraph.intrinsics.maxIntrinsicWidth}, lines=${layout.lineCount}")
            for (line in 0 until layout.lineCount - 1) {
                val end = layout.getLineEnd(line, visibleEnd = true)
                val text = layout.layoutInput.text.text
                assertTrue(text.getOrNull(end)?.isWhitespace() == true || text.getOrNull(end - 1)?.isWhitespace() == true, "Header word split")
            }
            val bounds = heading.getUnclippedBoundsInRoot()
            assertTrue(bounds.left >= viewport.left && bounds.right <= viewport.right)
        }
        onNodeWithTag("board-cell-doing").performScrollTo()
        val title = onNodeWithText(longTitle, useUnmergedTree = true).performScrollTo().assertIsDisplayed()
        assertTrue(textLayout(title).lineCount > 1, "Long title must wrap rather than widening all columns")
        assertFalse(textLayout(title).hasVisualOverflow)
    }
    @Test fun dropInEmptyLowerCellInRealShell() = dropInLowerCell(afterScroll = false)
    @Test fun dropAfterVerticalScrollingUsesRootCoordinates() = dropInLowerCell(afterScroll = true)

    private fun dropInLowerCell(afterScroll: Boolean) {
        assumeTrue(scale == 1f && theme == ThemeMode.Daylight && width in listOf(360, 800, 1920))
        screen(overflow = afterScroll && width != 360, height = if (width == 800) 600 else 800) { data ->
            val scroller = onNodeWithTag("board-columns")
            val card = onNodeWithTag("board-card-occ-card")
            if (width == 360) {
                // Whole-content stacked rows need a scroll to expose the adjacent destination.
                onNodeWithTag("board-cell-todo").performScrollTo()
            } else if (afterScroll) {
                val viewport = scroller.fetchSemanticsNode().boundsInRoot
                val full = card.getUnclippedBoundsInRoot()
                val delta = full.top.value - viewport.top + PorchlightTokens.touchTarget * 2
                scroller.performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, delta) }
            }
            val scroll = scroller.fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()
            if (afterScroll || width == 360) assertTrue(scroll > 0f, "Must actually scroll the Board")
            // Touch injection is relative to CLIPPED semantics bounds, not getUnclippedBoundsInRoot.
            val visible = card.fetchSemanticsNode().boundsInRoot
            val cell = onNodeWithTag("board-cell-todo").fetchSemanticsNode().boundsInRoot
            val emptyText = onNode(hasText("No cards") and hasAnyAncestor(hasTestTag("board-content-todo")), useUnmergedTree = true).getUnclippedBoundsInRoot()
            val release = Offset(cell.center.x, cell.bottom - PorchlightTokens.spaceInset)
            assertTrue(visible.height > 0 && cell.contains(release))
            assertTrue(release.y > cell.center.y && release.y > emptyText.bottom.value,
                "Release below empty content in the cell's lower half: $release / $cell / $emptyText")
            if (afterScroll && width != 360) {
                assertTrue(card.getUnclippedBoundsInRoot().top.value < visible.top, "Source must be clipped after scroll")
                assertTrue(scroll > cell.bottom - release.y, "Scroll offset exceeds bottom slack")
            }
            card.performTouchInput {
                down(Offset(visible.width / 2, minOf(visible.height / 2, PorchlightTokens.spaceInset.toFloat())))
                advanceEventTime(900)
                moveTo(release - visible.topLeft)
                up()
            }
            waitUntil(timeoutMillis = 10_000) { data.cache.view.value.board.first { it.id == "card" }.occurrence?.status == "todo" }
            onNodeWithText("To Do (1)").assertExists()
        }
    }
    @Test fun renderEvidence() {
        assumeTrue(width == 800 && theme == ThemeMode.Daylight && scale == 1f ||
            width in listOf(960, 1024) && theme == ThemeMode.Daylight && scale == 1f ||
            width == 1600 && theme == ThemeMode.Evening && scale == 1f ||
            width == 840 && theme == ThemeMode.Daylight && scale == 1f ||
            width == 360 && theme == ThemeMode.Daylight)
        screen(height = evidenceHeight()) {
            saveEvidence()
        }
    }
    private fun evidenceHeight() = when (width) { 800 -> 600; 360 -> 800; else -> 768 }
    private fun ComposeUiTest.saveEvidence(suffix: String = "", screen: String = "board") {
            val evidence = File("build/test-evidence/uifix-boardgrid").apply { mkdirs() }
            val phase = evidence.resolve("phase.txt").takeIf { it.exists() }?.readText()?.trim() ?: "after"
            require(phase in listOf("before", "after"))
            val pixels = onRoot().captureToImage().toPixelMap()
            val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
            for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
            assertTrue(ImageIO.write(image, "png", evidence.resolve("$screen-$phase-${width}dp-${platform.name.lowercase()}-${theme.name.lowercase()}-${scale.toInt()}x$suffix.png")))
    }
    @Test fun renderSelectedAndTodayEvidence() {
        assumeTrue(width == 1024 && scale == 1f && theme == ThemeMode.Daylight)
        screen(height = 768) {
            onNodeWithContentDescription("Open Fixture card").performScrollTo().performClick()
            onNodeWithTag("detail-pane").assertIsDisplayed()
            saveEvidence(suffix = "-selected")
            onNodeWithContentDescription("Navigate to Today").performClick()
            onNodeWithTag("detail-pane").assertDoesNotExist()
            saveEvidence(screen = "today")
        }
    }
}
