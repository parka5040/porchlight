package app.porchlight.shared

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.LocalScreenServices
import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.screens.board.BoardContent
import app.porchlight.shared.screens.board.boardColumns
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.runBlocking
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import java.awt.image.BufferedImage
import java.io.File
import java.util.concurrent.atomic.AtomicInteger
import javax.imageio.ImageIO
import kotlin.math.abs
import kotlin.test.*
import kotlin.time.Instant

/** Actual NavHost screens and callbacks. Harness dimensions/tolerances are dp at density 1. */
@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class ScreenPolishGeometryTest(private val widthDp: Int, private val scale: Float, private val theme: ThemeMode) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{0}dp-{1}x-{2}")
        fun cases(): List<Array<Any>> = listOf(320, 360, 412, 800, 1024).flatMap { width ->
            listOf(1f, 2f).flatMap { scale -> listOf(ThemeMode.Daylight, ThemeMode.Evening).map { arrayOf(width, scale, it) } }
        }
    }
    private val evidence = File("build/test-evidence/uifix-screens")
    private val searchCalls = AtomicInteger()
    private val searchQueries = java.util.concurrent.CopyOnWriteArrayList<String?>()
    private val keyboardHides = AtomicInteger()
    private val searchSucceeds = java.util.concurrent.atomic.AtomicBoolean(false)
    private val refreshFails = java.util.concurrent.atomic.AtomicBoolean(false)
    private fun screen(route: AppRoute, names: List<String> = listOf("Sam Rivera", "Alex Morgan"),
        listNames: List<String> = listOf("Groceries", "Hardware"),
        platform: PlatformKind = if (widthDp < PorchlightTokens.expandedBreakpoint) PlatformKind.Android else PlatformKind.Desktop,
        test: ComposeUiTest.() -> Unit) {
        val now = Instant.parse("2026-10-09T12:00:00Z")
        val cache = memoryCache { now }
        val members = listOf(
            MemberRead(false, "self", "household", "Sam Rivera", "user", "#FABD2F", "SR", "America/New_York", "manual", 1),
            MemberRead(false, "other", "household", "Alex Morgan", "user", "#8EC07C", "AM", "UTC", "manual", 1),
        ).take(names.size).mapIndexed { index, member -> member.copy(displayName = names[index]) }
        val lists = listNames.map { listFixture().copy(id = it.lowercase(), name = it, itemId = null, propertyEntries = emptyList(), gotIt = emptyList()) }
        val snapshot = emptySnapshot(1).copy(members = members,
            memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.overdue, TodaySection.today), SettingsScope.household, 7)))
        runBlocking { cache.install(snapshot); lists.forEach { cache.putList(it) } }
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            if (request.url.encodedPath == "/v1/lists" && refreshFails.get()) return@MockEngine respond("""{"status":503,"title":"Offline fixture"}""", HttpStatusCode.ServiceUnavailable, headersOf(HttpHeaders.ContentType, "application/problem+json"))
            if (request.url.encodedPath == "/v1/search" && searchSucceeds.get()) {
                searchCalls.incrementAndGet()
                return@MockEngine respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
            val body = when (request.url.encodedPath) {
                "/v1/board" -> """{"items":[],"next_cursor":null}"""
                "/v1/agenda" -> return@MockEngine respond("""{"status":503,"title":"Offline fixture"}""", HttpStatusCode.ServiceUnavailable, headersOf(HttpHeaders.ContentType, "application/problem+json"))
                "/v1/snapshot" -> WireJson.encodeToString(SnapshotRead.serializer(), snapshot)
                "/v1/me/settings" -> WireJson.encodeToString(SettingsRead.serializer(), SettingsRead("self", cache.view.value.settings!!.version + 1,
                    WireJson.decodeFromString(TodaySettings.serializer(), request.body.toByteArray().decodeToString())))
                "/v1/lists" -> """{"items":[${lists.joinToString(",") { WireJson.encodeToString(ListResponse.serializer(), it) }}],"next_cursor":null}"""
                "/v1/lists/groceries", "/v1/lists/hardware" -> WireJson.encodeToString(ListResponse.serializer(), lists.first { request.url.encodedPath.endsWith(it.id) })
                "/v1/search" -> { searchQueries += request.url.parameters["q"]; searchCalls.incrementAndGet(); return@MockEngine respond("""{"status":503,"title":"Offline fixture"}""", HttpStatusCode.ServiceUnavailable, headersOf(HttpHeaders.ContentType, "application/problem+json")) }
                else -> """{"items":[],"next_cursor":null}"""
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val clock = AppClock { now }
            override val screenSession = ScreenSession()
            override val platform = object : PlatformInfo {
                override val kind = platform
                override val systemDark = MutableStateFlow(theme == ThemeMode.Evening)
            }
        }
        try {
            runDesktopComposeUiTest(width = widthDp, height = 1000) {
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale), LocalScreenServices provides services,
                    androidx.compose.ui.platform.LocalSoftwareKeyboardController provides object : androidx.compose.ui.platform.SoftwareKeyboardController {
                        override fun show() = Unit
                        override fun hide() { keyboardHides.incrementAndGet() }
                    }) {
                    PorchlightTheme(theme) { Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                        ShellNavigation(route, DeviceSettings(), services.settings, {}, services = services)
                    } }
                } }
                val ready = when (route) {
                    is AppRoute.Calendar -> "Refresh calendar"
                    is AppRoute.Lists -> "Refresh lists"
                    else -> "New item"
                }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription(ready).fetchSemanticsNodes().isNotEmpty() }
                if (route is AppRoute.Calendar) waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Offline — showing saved data").fetchSemanticsNodes().isNotEmpty() }
                test()
            }
        } finally { services.screenSession.writeScope.cancel(); data.close(); api.close(); cache.close() }
    }
    private val DpRect.centerX: Float get() = (left.value + right.value) / 2
    private fun close(expected: Float, actual: Float, message: String) = assertTrue(abs(expected - actual) <= 1f, "$message: $expected / $actual dp")
    private fun ComposeUiTest.choice(label: String) = onNode(hasText(label) and hasClickAction() and SemanticsMatcher.keyIsDefined(SemanticsProperties.Selected))
    private fun ComposeUiTest.checkChoices(labels: List<String>) {
        val before = labels.associateWith { choice(it).getUnclippedBoundsInRoot() }
        labels.forEach { label ->
            val bounds = before.getValue(label)
            close(before.values.first().width.value, bounds.width.value, "$label equal segment width")
            assertTrue(bounds.width >= PorchlightTokens.touchTarget.dp && bounds.height >= PorchlightTokens.touchTarget.dp)
            if (!choice(label).fetchSemanticsNode().config[SemanticsProperties.Selected]) {
                val text = onNodeWithText(label, useUnmergedTree = true).getUnclippedBoundsInRoot()
                close(bounds.centerX, text.centerX, "$label unselected label centred")
            }
        }
        labels.forEach { selected ->
            choice(selected).performSemanticsAction(SemanticsActions.OnClick) { it() }
            waitUntil(timeoutMillis = 10_000) { choice(selected).fetchSemanticsNode().config[SemanticsProperties.Selected] }
            labels.forEach { assertEquals(before.getValue(it), choice(it).getUnclippedBoundsInRoot(), "$it moved selecting $selected") }
            val group = onNodeWithTag("choice-group-$selected", useUnmergedTree = true).getUnclippedBoundsInRoot()
            close(before.getValue(selected).centerX, group.centerX, "$selected check+label group centred")
        }
        labels.forEach { label ->
            val target = choice(label)
            if (generateSequence(target.fetchSemanticsNode().parent) { it.parent }.any { it.config.contains(SemanticsActions.ScrollBy) }) {
                target.performScrollTo()
            }
            target.assertIsDisplayed()
            assertTextFits(onNode(hasText(label) and hasAnyAncestor(hasClickAction() and SemanticsMatcher.keyIsDefined(SemanticsProperties.Selected)), useUnmergedTree = true))
        }
    }
    private fun assertTextFits(text: SemanticsNodeInteraction) {
        val layouts = mutableListOf<TextLayoutResult>()
        text.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        assertTrue(layouts.isNotEmpty())
        layouts.forEach { layout ->
            assertFalse(layout.didOverflowHeight)
            repeat(layout.lineCount) { line ->
                val width = if (layout.lineCount == 1) layout.multiParagraph.intrinsics.maxIntrinsicWidth else layout.getLineRight(line) - layout.getLineLeft(line)
                assertTrue(width <= layout.size.width + 1f, "Clipped ${layout.layoutInput.text}")
                assertFalse(layout.isLineEllipsized(line))
            }
        }
        val full = text.getUnclippedBoundsInRoot()
        val visible = text.fetchSemanticsNode().boundsInRoot
        close(full.width.value, visible.width, "Text visible width")
        close(full.height.value, visible.height, "Text visible height")
    }
    private fun ComposeUiTest.edge() = onNodeWithTag("shell-content").getUnclippedBoundsInRoot().left.value + PorchlightTokens.spaceMd
    private fun ComposeUiTest.aligned(vararg nodes: SemanticsNodeInteraction) { nodes.forEach { close(edge(), it.getUnclippedBoundsInRoot().left.value, "Content edge") } }

    @Test fun todaySegmentsAreEqualCentredAndStable() = screen(AppRoute.Today) { checkChoices(listOf("Mine", "Household")) }
    @Test fun boardSegmentsAreEqualCentredAndStable() = screen(AppRoute.Board()) {
        checkChoices(listOf("Mine", "Household"))
        checkChoices(listOf("Sam Rivera", "Alex Morgan"))
    }
    @Test fun calendarSegmentsAreEqualCentredAndStable() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        checkChoices(if (widthDp < PorchlightTokens.expandedBreakpoint) listOf("Day", "Week", "Agenda") else listOf("Day", "Week", "Agenda", "Month"))
    }
    @Test fun listsSegmentsAreEqualCentredAndStable() = screen(AppRoute.Lists()) { checkChoices(listOf("Groceries", "Hardware")) }

    /** One Board scroll reveals the requested content; cells never scroll independently. */
    private fun ComposeUiTest.revealBoardColumn(target: SemanticsNodeInteraction) {
        target.performScrollTo()
        onNodeWithTag("board-columns").assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.HorizontalScrollAxisRange))
    }
    @Test fun boardColumnsHaveVisibleCountedHeadersAndAreReachable() = screen(AppRoute.Board()) {
        aligned(choice("Mine"), onNodeWithContentDescription("New item"), onNodeWithText("Backlog (0)"))
        val across = if (widthDp in listOf(800, 1024)) 2 else 1
        val bounds = listOf("Backlog", "To Do", "Doing", "Done").map { onNodeWithText("$it (0)").getUnclippedBoundsInRoot() }
        bounds.chunked(across).forEach { row ->
            close(edge(), row.first().left.value, "Each Board row starts at the content edge")
            row.zipWithNext().forEach { (left, right) ->
                close(left.top.value, right.top.value, "Headers in the same grid row align")
                assertTrue(right.left > left.right)
            }
        }
        listOf("Backlog", "To Do", "Doing", "Done").forEach { label ->
            val header = onNodeWithText("$label (0)")
            revealBoardColumn(header)
            header.assertIsDisplayed()
            if (across == 1) aligned(header)
            assertTextFits(header)
        }
    }
    @Test fun populatedBoardAdaptsToPaneWidthAndAlignsCardsWithCountedHeaders() = runDesktopComposeUiTest(width = widthDp, height = 1000) {
        val statuses = listOf("backlog", "todo", "doing", "done")
        val rows = statuses.map { status -> itemRead(status).copy(title = "Task $status", capabilities = listOf("view"),
            occurrence = occurrenceRead("occ-$status", status).copy(status = status)) }
        val services = object : AppServices by FakeServices() {
            override val platform = object : PlatformInfo {
                override val kind = if (widthDp < PorchlightTokens.expandedBreakpoint) PlatformKind.Android else PlatformKind.Desktop
                override val systemDark = MutableStateFlow(theme == ThemeMode.Evening)
            }
        }
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale), LocalScreenServices provides services) { PorchlightTheme(theme) {
            BoardContent(boardColumns(rows, null, "self"), emptyList(), {}, { _, _ -> })
        } } }
        val across = when (widthDp) { 1024 -> 4; 800 -> 2; else -> 1 }
        val before = statuses.map { onNodeWithTag("board-heading-$it").getUnclippedBoundsInRoot() }
        before.chunked(across).forEach { row -> row.zipWithNext().forEach { (first, next) ->
            close(first.top.value, next.top.value, "Grid row header tops")
            assertTrue(next.left > first.right, "Columns in the same row are adjacent")
        } }
        before.chunked(across).zipWithNext().forEach { (first, next) -> assertTrue(next.first().top > first.first().bottom, "Rows stack vertically") }
        statuses.forEach { status ->
            val heading = onNodeWithTag("board-heading-$status")
            revealBoardColumn(onNodeWithTag("board-card-occ-$status"))
            heading.performScrollTo().assertIsDisplayed()
            heading.assertTextEquals(mapOf("backlog" to "Backlog (1)", "todo" to "To Do (1)", "doing" to "Doing (1)", "done" to "Done (1)").getValue(status))
            assertTextFits(heading)
            val card = onNodeWithTag("board-card-occ-$status")
            val headerBounds = heading.getUnclippedBoundsInRoot()
            val cardBounds = card.getUnclippedBoundsInRoot()
            close(headerBounds.left.value, cardBounds.left.value, "Column/card edge")
            assertTrue(cardBounds.right.value <= widthDp, "Card outside pane")
            if (across == 1) close(0f, headerBounds.left.value, "Narrow column edge")
        }
    }

    private fun ComposeUiTest.assertIconPainted(description: String) {
        val image = onNodeWithContentDescription(description).captureToImage().toPixelMap()
        val ink = (if (theme == ThemeMode.Daylight) PorchlightTokens.Daylight else PorchlightTokens.Evening).onSurface.toArgb()
        // Exclude the outline: icon ink must occur inside the central half of the touch target.
        assertTrue((image.width / 4 until image.width * 3 / 4).any { x ->
            (image.height / 4 until image.height * 3 / 4).any { y -> image[x, y].toArgb() == ink }
        }, "$description must paint its icon, not just an empty button")
    }

    @Test fun calendarHeaderPeriodNavigationAndMemberRowsAreAligned() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        onNodeWithText("Calendar · ", substring = true).assertDoesNotExist()
        aligned(choice("Day"), onNodeWithText("Oct 4 – 10, 2026"), onNodeWithText("New York time"),
            onNodeWithContentDescription("Previous week"), onNodeWithContentDescription("Refresh calendar"), onNodeWithText("Show calendars"))
        onNodeWithContentDescription("Next week").assertExists()
        assertIconPainted("Previous week")
        assertIconPainted("Next week")
        val previous = onNodeWithContentDescription("Previous week").getUnclippedBoundsInRoot()
        val today = onNodeWithContentDescription("Today").getUnclippedBoundsInRoot()
        val next = onNodeWithContentDescription("Next week").getUnclippedBoundsInRoot()
        close(previous.top.value, today.top.value, "Period navigation is one row")
        close(previous.top.value, next.top.value, "Period navigation is one row")
        close(previous.height.value, today.height.value, "Navigation touch surfaces align")
        assertTrue(previous.right < today.left && today.right < next.left)
        listOf("Sam Rivera", "Alex Morgan").forEach { name ->
            onNodeWithText("Show $name").assertDoesNotExist()
            val member = onNodeWithContentDescription("Calendar for $name").performScrollTo().assertHasClickAction()
            aligned(member)
            if (name == "Sam Rivera") member.assertIsSelected() else member.assertIsNotSelected()
            onAllNodesWithText(name, useUnmergedTree = true).assertCountEquals(1)
        }
        val retry = onNodeWithContentDescription("Try again: retry loading").performScrollTo()
        aligned(retry)
        // The segment has a horizontal scroll parent inside the screen's vertical scroll parent.
        onNodeWithTag("calendar-scroll").performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, -10000f) }
        choice("Day").performScrollTo().assertIsDisplayed().performClick()
        onNodeWithText("Fri, Oct 9, 2026").assertExists()
        onNodeWithContentDescription("Previous day").assertExists()
        onNodeWithContentDescription("Next day").assertExists()
        choice("Agenda").performScrollTo().performClick()
        onNodeWithText("Oct 9 – 15, 2026").assertExists()
        onNodeWithContentDescription("Previous 7 days").assertExists()
        onNodeWithContentDescription("Next 7 days").assertExists()
    }
    @Test fun listsSearchUsesImeAndHouseholdConnectionWording() = screen(AppRoute.Lists()) {
        onNodeWithText("online", substring = true, ignoreCase = true).assertDoesNotExist()
        // Icons-only compact navigation retains its accessible name, not a clipped text label.
        onAllNodesWithText("Lists", useUnmergedTree = true).assertCountEquals(if (widthDp == 320 && scale == 2f) 1 else 2)
        onNodeWithContentDescription("Navigate to Lists").assertIsDisplayed()
        val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
        assertEquals(ImeAction.Search, field.fetchSemanticsNode().config[SemanticsProperties.ImeAction])
        aligned(onNodeWithContentDescription("Refresh lists"), choice("Groceries"), field, onNodeWithText("New list name"), onNodeWithContentDescription("Create list"))
        onNode(hasText("Search lists") and hasClickAction() and !hasSetTextAction()).assertDoesNotExist()
        assertIconPainted("Search lists")
        field.performTextInput("milk")
        field.performImeAction()
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 1 }
        assertEquals(listOf("milk"), searchQueries.toList())
        waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Search needs a connection to your household server.").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Search lists").performClick()
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 2 }
    }
    @Test fun boardgridDesktopUsesTwoRowsWhenFourDoNotFit() {
        assumeTrue(widthDp == 1024 || widthDp == 800)
        screen(AppRoute.Board(), platform = PlatformKind.Desktop) {
            val bounds = listOf("backlog", "todo", "doing", "done").map { onNodeWithTag("board-heading-$it").getUnclippedBoundsInRoot() }
            bounds.chunked(2).forEach { (left, right) ->
                close(left.top.value, right.top.value, "Two complete adjacent columns per row")
                assertTrue(right.left > left.right)
            }
            assertTrue(bounds[2].top > bounds[0].bottom)
            close(bounds[0].left.value, bounds[2].left.value, "Grid rows align")
        }
    }

    private fun ComposeUiTest.assertWholeSegments(labels: List<String>) {
        val left = edge()
        val right = onNodeWithTag("shell-content").getUnclippedBoundsInRoot().right.value - PorchlightTokens.spaceMd
        labels.forEach { label ->
            val node = choice(label)
            val bounds = node.getUnclippedBoundsInRoot()
            assertTrue(bounds.left.value >= left - 1 && bounds.right.value <= right + 1,
                "$label segment must be fully inside viewport: $bounds, viewport=$left..$right")
            node.assertIsDisplayed()
            val visible = node.fetchSemanticsNode().boundsInRoot
            close(bounds.width.value, visible.width, "$label entire segment visible width")
            close(bounds.height.value, visible.height, "$label entire segment visible height")
            val text = onNodeWithText(label, useUnmergedTree = true)
            val layouts = mutableListOf<TextLayoutResult>()
            text.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
            layouts.forEach { layout ->
                assertFalse(layout.hasVisualOverflow, "$label has visual overflow: size=${layout.size}, width=${layout.didOverflowWidth}, height=${layout.didOverflowHeight}, intrinsic=${layout.multiParagraph.intrinsics.maxIntrinsicWidth}, paragraph=${layout.multiParagraph.width}")
                for (line in 0 until layout.lineCount - 1) {
                    val end = layout.getLineEnd(line, visibleEnd = true)
                    assertTrue(end == label.length || label.getOrNull(end)?.isWhitespace() == true || label.getOrNull(end - 1)?.isWhitespace() == true,
                        "$label must wrap only at word boundaries (line $line ends at $end)")
                }
            }
        }
    }
    @Test fun fix2TodaySegmentsAreWhole() = screen(AppRoute.Today) { assertWholeSegments(listOf("Mine", "Household")) }
    @Test fun fix2CalendarSegmentsAreWhole() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        assertWholeSegments(if (widthDp < PorchlightTokens.expandedBreakpoint) listOf("Day", "Week", "Agenda") else listOf("Day", "Week", "Agenda", "Month"))
    }
    @Test fun fix2ListsFourSegmentsAreWhole() = screen(AppRoute.Lists(), listNames = listOf("Groceries", "Hardware", "Weekend jobs", "Pantry")) {
        assertWholeSegments(listOf("Groceries", "Hardware", "Weekend jobs", "Pantry"))
    }
    @Test fun fix2BoardScopeAndMemberSegmentsAreWhole() = screen(AppRoute.Board(), names = listOf("Fixture Representative", "Household Coordinator")) {
        assertWholeSegments(listOf("Mine", "Household", "Fixture Representative", "Household Coordinator"))
    }
    @Test fun fix2ShellUsesFullLabelsBeforeEllipsizing() {
        assumeTrue(scale == 2f && widthDp in listOf(320, 360))
        screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
            val title = onNode(hasText("Calendar") and !hasAnyAncestor(hasContentDescription("Navigate to Calendar")), useUnmergedTree = true)
            if (widthDp == 320) mainDestinations.forEach { (label, _) ->
                onNodeWithTag("nav-label-$label", useUnmergedTree = true).assertDoesNotExist()
                onNodeWithContentDescription("Navigate to $label").assertIsDisplayed().assertHasClickAction()
            }
            (listOf("title" to title) + if (widthDp == 320) emptyList() else mainDestinations.map { (label, _) -> label to onNodeWithTag("nav-label-$label", useUnmergedTree = true) }).forEach { (label, node) ->
                val layouts = mutableListOf<TextLayoutResult>()
                node.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                layouts.forEach { layout ->
                    assertEquals(1, layout.lineCount)
                    val full = layout.multiParagraph.intrinsics.maxIntrinsicWidth <= layout.size.width + 1f && !layout.hasVisualOverflow
                    println("shell $widthDp dp 2x $label: full=$full style=${layout.layoutInput.style.fontSize} required=${layout.multiParagraph.intrinsics.maxIntrinsicWidth} available=${layout.size.width}")
                    if (widthDp == 360) assertTrue(full, "$label must render in full at 360dp 2x")
                }
            }
        }
    }
    @Test fun fix2SearchErrorIsDirectlyBelowField() = screen(AppRoute.Lists()) {
        val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
        field.performTextInput("milk")
        field.performImeAction()
        val message = "Search needs a connection to your household server."
        waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(message).fetchSemanticsNodes().isNotEmpty() }
        assertEquals(listOf("milk"), searchQueries.toList())
        val error = onNodeWithText(message).getUnclippedBoundsInRoot()
        assertTrue(error.top >= field.getUnclippedBoundsInRoot().bottom, "Search error belongs below its field")
    }
    @Test fun fix2SearchHidesKeyboardForImeAndIcon() = screen(AppRoute.Lists()) {
        val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
        field.performTextInput("milk")
        field.performImeAction()
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 1 }
        assertEquals(1, keyboardHides.get(), "IME search must hide keyboard")
        onNodeWithContentDescription("Search lists").performSemanticsAction(SemanticsActions.OnClick) { it() }
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 2 }
        assertEquals(2, keyboardHides.get(), "Icon search must hide keyboard too")
        assertEquals(listOf("milk", "milk"), searchQueries.toList())
    }
    @Test fun boardgridSettingsHasOneStableDescription() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        onNodeWithContentDescription("Settings").assertIsDisplayed().assertHasClickAction()
        onNodeWithContentDescription("Open settings").assertDoesNotExist()
    }
    @Test fun boardgridCompactNavigationKeepsTouchFloor() {
        assumeTrue(widthDp < PorchlightTokens.expandedBreakpoint)
        screen(AppRoute.Today) {
            val targets = mainDestinations.map { (label, _) ->
                onNodeWithContentDescription("Navigate to $label").assertIsDisplayed().getUnclippedBoundsInRoot().also { bounds ->
                    assertTrue(bounds.width >= PorchlightTokens.touchTarget.dp, "$label width ${bounds.width}")
                    assertTrue(bounds.left >= 0.dp && bounds.right <= widthDp.dp, "$label must be inside viewport")
                }
            }
            targets.zipWithNext().forEach { (left, right) -> assertTrue(left.right <= right.left, "Touch targets must not overlap") }
        }
    }
    @Test fun boardgridShellFullLabelsAtNormalScale() {
        assumeTrue(scale == 1f && widthDp in listOf(360, 412))
        screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
            val title = onNode(hasText("Calendar") and !hasAnyAncestor(hasContentDescription("Navigate to Calendar")), useUnmergedTree = true)
            (listOf(title) + mainDestinations.map { (label, _) -> onNodeWithTag("nav-label-$label", useUnmergedTree = true) }).forEach(::assertTextFits)
        }
    }
    @Test fun boardgridSearchErrorClearsOnEditIncludingEmpty() {
        assumeTrue(widthDp == 360 && scale == 1f && theme == ThemeMode.Daylight)
        screen(AppRoute.Lists()) {
            val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
            field.performTextInput("milk")
            field.performImeAction()
            val error = "Search needs a connection to your household server."
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(error).fetchSemanticsNodes().isNotEmpty() }
            field.performTextReplacement("bread")
            onNodeWithText(error).assertDoesNotExist()
            field.performImeAction()
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(error).fetchSemanticsNodes().isNotEmpty() }
            field.performTextClearance()
            onNodeWithText(error).assertDoesNotExist()
        }
    }
    @Test fun boardgridSuccessfulSearchClearsSearchAndRefreshErrors() {
        assumeTrue(widthDp == 360 && scale == 1f && theme == ThemeMode.Daylight)
        screen(AppRoute.Lists()) {
            refreshFails.set(true)
            onNodeWithContentDescription("Refresh lists").performClick()
            val refreshError = "Could not refresh lists. Cached rows may be stale."
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(refreshError).fetchSemanticsNodes().isNotEmpty() }
            val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
            field.performTextInput("milk")
            field.performImeAction()
            val searchError = "Search needs a connection to your household server."
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(searchError).fetchSemanticsNodes().isNotEmpty() }
            searchSucceeds.set(true)
            field.performImeAction()
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(searchError).fetchSemanticsNodes().isEmpty() }
            onNodeWithText(refreshError).assertDoesNotExist()
        }
    }
    private val longNames = listOf("Fixture Representative-Primary", "Household Coordinator-Secondary")

    @Test fun fix1BoardOneLongMemberHasItsOwnAlignedRow() = checkBoardMemberRows(longNames.take(1))
    @Test fun fix1BoardTwoLongMembersFitWithoutClipping() = checkBoardMemberRows(longNames)

    private fun checkBoardMemberRows(names: List<String>) = screen(AppRoute.Board(), names) {
        val mine = choice("Mine").getUnclippedBoundsInRoot()
        val household = choice("Household").getUnclippedBoundsInRoot()
        if (scale == 1f) close(mine.top.value, household.top.value, "Mine / Household share the first row when words fit")
        close(mine.width.value, household.width.value, "Scope segments stay equal")
        if (scale == 1f) {
            listOf("Mine", "Household").forEach { label ->
                val bounds = choice(label).assertIsDisplayed().getUnclippedBoundsInRoot()
                assertTrue(bounds.left.value >= edge() && bounds.right.value <= widthDp - PorchlightTokens.spaceMd,
                    "$label must fit without a partially visible segment")
            }
        }
        names.forEach { name ->
            val member = onNodeWithContentDescription("Filter board for $name").assertIsDisplayed()
            val bounds = member.getUnclippedBoundsInRoot()
            assertTrue(bounds.top >= mine.bottom, "$name belongs BELOW Mine / Household, not in their row")
            assertTrue(bounds.left.value >= edge() && bounds.right.value <= widthDp - PorchlightTokens.spaceMd,
                "$name must fit or wrap within the member row")
            assertTrue(bounds.width >= PorchlightTokens.touchTarget.dp && bounds.height >= PorchlightTokens.touchTarget.dp)
            assertTextFits(onNodeWithText(name, useUnmergedTree = true))
        }
        aligned(choice(names.minBy { choice(it).getUnclippedBoundsInRoot().left.value }))
        val labels = listOf("Mine", "Household") + names
        val before = labels.associateWith { choice(it).getUnclippedBoundsInRoot() }
        labels.forEach { selected ->
            choice(selected).performSemanticsAction(SemanticsActions.OnClick) { it() }
            waitUntil(timeoutMillis = 10_000) { choice(selected).fetchSemanticsNode().config[SemanticsProperties.Selected] }
            labels.forEach { label ->
                assertEquals(label == selected, choice(label).fetchSemanticsNode().config[SemanticsProperties.Selected],
                    "Exactly one Board filter, including across both rows")
                assertEquals(before.getValue(label), choice(label).getUnclippedBoundsInRoot(), "Selecting $selected moved $label")
            }
        }
    }

    @Test fun fix1ListsSearchIconIsInsideFieldAndHasNoOutline() = screen(AppRoute.Lists()) {
        val field = onNode(hasText("Search lists") and hasSetTextAction()).performScrollTo()
        val icon = onNodeWithContentDescription("Search lists").assertHasClickAction().assertIsDisplayed()
        val outer = field.getUnclippedBoundsInRoot()
        val inner = icon.getUnclippedBoundsInRoot()
        assertTrue(inner.left >= outer.left && inner.right <= outer.right && inner.top >= outer.top && inner.bottom <= outer.bottom,
            "Search icon must be inside the field")
        icon.assert(hasAnyAncestor(hasSetTextAction()))
        assertTrue(inner.width >= PorchlightTokens.touchTarget.dp && inner.height >= PorchlightTokens.touchTarget.dp)
        onNode(hasText("Search lists") and hasClickAction() and !hasSetTextAction()).assertDoesNotExist()
        // A separate outlined button paints a horizontal line across its top inset. The parent's
        // field outline is OUTSIDE this central-half sample; a borderless icon leaves it clear.
        val image = icon.captureToImage().toPixelMap()
        val outline = (if (theme == ThemeMode.Daylight) PorchlightTokens.Daylight else PorchlightTokens.Evening).outline.toArgb()
        val hasSeparateOutline = (0 until PorchlightTokens.spaceXs).any { y ->
            (image.width / 4 until image.width * 3 / 4).all { x -> image[x, y].toArgb() == outline }
        }
        assertFalse(hasSeparateOutline, "Search lists must not draw a separate outlined box inside the field")
        assertIconPainted("Search lists")
        field.performTextInput("milk")
        field.performImeAction()
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 1 }
        assertEquals(listOf("milk"), searchQueries.toList())
        waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Search needs a connection to your household server.").fetchSemanticsNodes().isNotEmpty() }
        field.performScrollTo()
        icon.performClick()
        waitUntil(timeoutMillis = 10_000) { searchCalls.get() == 2 }
    }

    @Test fun fix1CalendarDayHeadingUsesHumanDate() {
        assumeTrue(widthDp < PorchlightTokens.expandedBreakpoint)
        screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
            onNodeWithText("Sunday, Oct 4").performScrollTo().assertIsDisplayed()
            onNodeWithText("Sunday · 2026-10-04").assertDoesNotExist()
            onNodeWithText("Sunday, Oct 4, 2026").assertDoesNotExist()
        }
    }

    @Test fun fix1CalendarCrossYearDayHeadingsIncludeYear() {
        assumeTrue(widthDp < PorchlightTokens.expandedBreakpoint)
        screen(AppRoute.Calendar(CalendarView.Week, "2027-01-01")) {
            onNodeWithText("Dec 27, 2026 – Jan 2, 2027").assertExists()
            onNodeWithText("Sunday, Dec 27, 2026").performScrollTo().assertIsDisplayed()
            onNodeWithText("Friday, Jan 1, 2027").performScrollTo().assertIsDisplayed()
        }
    }

    private fun assertSingleLineOrEllipsized(text: SemanticsNodeInteraction) {
        val layouts = mutableListOf<TextLayoutResult>()
        text.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
        assertTrue(layouts.isNotEmpty())
        layouts.forEach { layout ->
            assertEquals(1, layout.lineCount, "${layout.layoutInput.text} must never split across lines")
            // Skia's semantics result reports maxLines truncation as height overflow and does not
            // reliably expose isLineEllipsized. Verify the one-line layout and its actual overflow policy.
            assertEquals(androidx.compose.ui.text.style.TextOverflow.Ellipsis, layout.layoutInput.overflow)
            assertEquals(1, layout.layoutInput.maxLines)
        }
    }

    @Test fun fix1ShellCalendarTitleNeverSplitsAndSettingsStaysVisible() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        val title = onNode(hasText("Calendar") and !hasAnyAncestor(hasContentDescription("Navigate to Calendar")), useUnmergedTree = true)
        assertSingleLineOrEllipsized(title)
        val settings = onNodeWithContentDescription("Settings").assertIsDisplayed().assertHasClickAction().getUnclippedBoundsInRoot()
        assertTrue(settings.width >= PorchlightTokens.touchTarget.dp && settings.height >= PorchlightTokens.touchTarget.dp)
        if (widthDp < PorchlightTokens.expandedBreakpoint) assertTrue(title.getUnclippedBoundsInRoot().right <= settings.left)
    }

    @Test fun fix1ShellNavigationLabelsNeverSplitWords() = screen(AppRoute.Calendar(CalendarView.Week, "2026-10-09")) {
        mainDestinations.forEach { (label, _) ->
            if (widthDp == 320 && scale == 2f) onNodeWithTag("nav-label-$label", useUnmergedTree = true).assertDoesNotExist()
            else assertSingleLineOrEllipsized(onNodeWithTag("nav-label-$label", useUnmergedTree = true))
            val target = onNodeWithContentDescription("Navigate to $label").assertIsDisplayed().assertHasClickAction().getUnclippedBoundsInRoot()
            assertTrue(target.width >= PorchlightTokens.touchTarget.dp && target.height >= PorchlightTokens.touchTarget.dp)
        }
    }

    private fun ComposeUiTest.png(screen: String) {
        evidence.mkdirs()
        val phase = evidence.resolve("phase.txt").takeIf { it.exists() }?.readText()?.trim() ?: "after"
        require(phase in listOf("before", "after"))
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        assertTrue(ImageIO.write(image, "png", evidence.resolve("$screen-$phase-${widthDp}dp-${theme.name.lowercase()}-${scale.toInt()}x.png")))
    }
    private fun evidenceCase(screen: String) = widthDp == 360 && theme == ThemeMode.Daylight ||
        widthDp == 412 && theme == ThemeMode.Evening && scale == 1f ||
        screen == "board" && widthDp == 1024 && theme == ThemeMode.Daylight && scale == 1f
    private fun render(route: AppRoute, name: String, names: List<String> = listOf("Sam Rivera", "Alex Morgan")) {
        assumeTrue("PNG does not apply to this parameter set", evidenceCase(name))
        screen(route, names) { png(name) }
    }
    @Test fun renderToday() = render(AppRoute.Today, "today")
    @Test fun renderBoard() = render(AppRoute.Board(), "board")
    @Test fun renderCalendarWeek() = render(AppRoute.Calendar(CalendarView.Week, "2026-10-09"), "calendar-week")
    @Test fun renderCalendarDay() = render(AppRoute.Calendar(CalendarView.Day, "2026-10-09"), "calendar-day")
    @Test fun renderLists() = render(AppRoute.Lists(), "lists")
    @Test fun renderBoardOneLongMember() = render(AppRoute.Board(), "board-1-long-members", longNames.take(1))
    @Test fun renderBoardTwoLongMembers() = render(AppRoute.Board(), "board-2-long-members", longNames)
}
