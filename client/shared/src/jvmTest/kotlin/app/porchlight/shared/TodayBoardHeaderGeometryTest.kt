package app.porchlight.shared

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.navigation.ShellNavigation
import app.porchlight.shared.screens.LocalScreenServices
import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.settings.DeviceSettings
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.theme.PorchlightTokens
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.runners.Parameterized
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
import kotlin.math.abs
import kotlin.test.*
import kotlin.time.Instant

/** Real screens, real shell/title and real scope callbacks; only the network/cache are fixtures.
 * Density is 1 px/dp in the offscreen harness, so all sizes/tolerances below are dp, not device pixels.
 */
@OptIn(ExperimentalTestApi::class)
@RunWith(Parameterized::class)
class TodayBoardHeaderGeometryTest(
    private val widthDp: Int,
    private val fontScale: Float,
    private val theme: ThemeMode,
    private val screen: String,
) {
    companion object {
        @JvmStatic @Parameterized.Parameters(name = "{3}-{0}dp-{1}x-{2}")
        fun cases(): List<Array<Any>> = listOf(320, 360, 412, 1024).flatMap { width ->
            listOf(1f, 2f).flatMap { scale -> listOf(ThemeMode.Daylight, ThemeMode.Evening).flatMap { theme ->
                listOf("Today", "Board").map { screen -> arrayOf(width, scale, theme, screen) }
            } }
        }
    }

    private val palette get() = if (theme == ThemeMode.Daylight) PorchlightTokens.Daylight else PorchlightTokens.Evening
    private val memberNames = listOf("Alex", "Sam", "Alexandra Montgomery-Rivera")
    private val options get() = listOf("Mine", "Household") + if (screen == "Board") memberNames else emptyList()
    private val actions = listOf("New item", "Refresh")
    // Regenerable render evidence stays in the build tree, not var/.
    private val evidenceDir get() = File("build/test-evidence/todayheader")

    private fun header(test: ComposeUiTest.() -> Unit) {
        val now = Instant.parse("2026-10-09T12:00:00Z")
        val cache = memoryCache { now }
        val snapshot = emptySnapshot(1).copy(
            memberSettings = SettingsRead("self", 1, TodaySettings(listOf(TodaySection.overdue, TodaySection.today), SettingsScope.household, 7)),
            members = memberNames.mapIndexed { index, name ->
                MemberRead(false, "member-$index", "household", name, "user", "#FABD2F", name.take(1), "UTC", "manual", 1)
            },
        )
        runBlocking { cache.install(snapshot) }
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            val body = when (request.url.encodedPath) {
                "/v1/board", "/v1/agenda" -> """{"items":[],"next_cursor":null}"""
                "/v1/snapshot" -> WireJson.encodeToString(SnapshotRead.serializer(), snapshot)
                "/v1/me/settings" -> WireJson.encodeToString(SettingsRead.serializer(), SettingsRead("self",
                    cache.view.value.settings!!.version + 1,
                    WireJson.decodeFromString(TodaySettings.serializer(), request.body.toByteArray().decodeToString())))
                else -> error("Unexpected fixture request: ${request.url.encodedPath}")
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val data = HouseholdData(cache, GeneratedCall({ api }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val clock = AppClock { now }
            override val screenSession = ScreenSession()
            override val platform = object : PlatformInfo {
                override val kind = if (widthDp < PorchlightTokens.expandedBreakpoint) PlatformKind.Android else PlatformKind.Desktop
                override val systemDark = kotlinx.coroutines.flow.MutableStateFlow(theme == ThemeMode.Evening)
            }
        }
        try {
            runDesktopComposeUiTest(width = widthDp, height = 1000) {
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, fontScale), LocalScreenServices provides services) {
                        PorchlightTheme(theme) {
                            Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                                ShellNavigation(if (screen == "Today") AppRoute.Today else AppRoute.Board(),
                                    DeviceSettings(), services.settings, {}, services = services)
                            }
                        }
                    }
                }
                waitUntil(timeoutMillis = 10_000) { onAllNodes(hasText("New item") and hasClickAction()).fetchSemanticsNodes().isNotEmpty() }
                test()
            }
        } finally {
            services.screenSession.writeScope.cancel()
            data.close()
            api.close()
            cache.close()
        }
    }

    private fun ComposeUiTest.control(label: String) = onNode(hasText(label) and hasClickAction())
    private fun ComposeUiTest.bounds(label: String) = control(label).getUnclippedBoundsInRoot()
    private fun close(expected: Float, actual: Float, what: String) =
        assertTrue(abs(expected - actual) <= 1.dp.value, "$what: expected $expected dp, got $actual dp")

    private fun ComposeUiTest.contentEdge(): Float {
        val title = onAllNodesWithText(screen, useUnmergedTree = true).fetchSemanticsNodes().minBy { it.boundsInRoot.top }
        val section = onNodeWithText(if (screen == "Today") "Overdue" else "Backlog (0)")
        close(title.boundsInRoot.left, section.fetchSemanticsNode().boundsInRoot.left, "Title / section edge")
        return title.boundsInRoot.left
    }

    /** Unlike semantics-only bounds, this catches transparent TextButton padding and empty check slots. */
    private fun ComposeUiTest.visibleLeft(label: String): Float {
        val image = control(label).captureToImage().toPixelMap()
        val background = palette.background.toArgb()
        val firstPaint = (0 until image.width).firstOrNull { x ->
            (image.height / 4 until image.height * 3 / 4).any { y -> image[x, y].toArgb() != background }
        }
        assertNotNull(firstPaint, "$label must paint a visible control")
        return bounds(label).left.value + firstPaint
    }

    @Test fun visibleScopeAndPrimaryEdgesMatchTitleAndContent() = header {
        val edge = contentEdge()
        // The first option, whichever order the screen uses, must have a visible container in BOTH states.
        val first = options.minBy { bounds(it).left.value }
        close(edge, visibleLeft(first), "First scope visible edge")
        close(edge, visibleLeft("New item"), "First action visible edge")
        control(first).performClick()
        waitUntil(timeoutMillis = 10_000) { control(first).fetchSemanticsNode().config[SemanticsProperties.Selected] }
        close(edge, visibleLeft(first), "Selected scope visible edge")
    }

    @Test fun actionsHaveEqualHeightTopTokenGapAndVisualHierarchy() = header {
        val primary = bounds("New item")
        val secondary = bounds("Refresh")
        close(primary.top.value, secondary.top.value, "Action top")
        close(primary.height.value, secondary.height.value, "Action height")
        close(PorchlightTokens.spaceSm.toFloat(), secondary.left.value - primary.right.value, "Action gap")
        for (label in actions) {
            val node = control(label).assertHasClickAction().assertIsDisplayed().fetchSemanticsNode()
            assertFalse(node.config.contains(SemanticsProperties.Selected), "An action is not a scope")
            assertTouchTarget(bounds(label), label)
        }
        val primaryImage = control("New item").captureToImage().toPixelMap()
        val secondaryImage = control("Refresh").captureToImage().toPixelMap()
        // Sample inside the straight top edge, above text: a filled primary vs an outlined secondary.
        assertEquals(palette.primary.toArgb(), primaryImage[primaryImage.width / 2, PorchlightTokens.spaceXs].toArgb())
        assertEquals(palette.background.toArgb(), secondaryImage[secondaryImage.width / 2, PorchlightTokens.spaceXs].toArgb())
    }

    private fun assertTouchTarget(rect: DpRect, label: String) {
        assertTrue(rect.width >= PorchlightTokens.touchTarget.dp && rect.height >= PorchlightTokens.touchTarget.dp,
            "$label target: $rect")
    }

    @Test fun scopeRowsHaveEqualSegmentsAndNeverMoveOnSelection() = header {
        val before = options.associateWith { bounds(it) }
        val first = before.getValue("Mine")
        options.forEach { label ->
            val rect = before.getValue(label)
            val groupFirst = if (label in memberNames) before.getValue(memberNames.first()) else first
            if (label in memberNames) assertTrue(rect.top >= first.bottom, "$label belongs to a separate member row")
            else if (rect.top != first.top) {
                assertTrue(rect.top >= first.bottom, "$label flows below the first scope when words cannot share a row")
                close(first.left.value, rect.left.value, "$label flowed scope left edge")
            }
            close(groupFirst.width.value, rect.width.value, "$label equal segment width within its control")
            close(groupFirst.height.value, rect.height.value, "$label equal segment height within its control")
            assertTouchTarget(rect, label)
            if (label == "Household") control(label).assertIsSelected() else control(label).assertIsNotSelected()
        }
        options.forEach { selected ->
            // Invoke the real callback without scrolling: compare every option at the same scroll origin.
            control(selected).performSemanticsAction(SemanticsActions.OnClick) { it() }
            waitUntil(timeoutMillis = 10_000) { control(selected).fetchSemanticsNode().config[SemanticsProperties.Selected] }
            options.forEach { label ->
                if (label == selected) control(label).assertIsSelected() else control(label).assertIsNotSelected()
                assertEquals(before.getValue(label), bounds(label), "$label moved on selecting $selected")
            }
            onNodeWithTag("selection-$selected", useUnmergedTree = true).assertExists()
        }
    }

    @Test fun everyLabelFitsItsTargetAndIsReachableAtLargeFonts() = header {
        (options + actions).forEach { label ->
            val target = control(label)
            // Scope/member controls flow onto aligned rows; other screen ancestors may scroll.
            if (label in options && target.fetchSemanticsNode().parent != null) {
                val hasScrollAncestor = generateSequence(target.fetchSemanticsNode().parent) { it.parent }
                    .any { it.config.contains(SemanticsActions.ScrollBy) }
                if (hasScrollAncestor) target.performScrollTo()
            }
            target.assertIsDisplayed()
            val text = onNodeWithText(label, useUnmergedTree = true)
            val layouts = mutableListOf<TextLayoutResult>()
            text.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
            assertTrue(layouts.isNotEmpty())
            layouts.forEach { layout ->
                // Skia's semantics result can position centered lines in the incoming maxWidth rather
                // than the final wrap-content width. Check line *lengths* plus actual clipped bounds below.
                assertFalse(layout.didOverflowHeight, "$label vertical text overflow")
                repeat(layout.lineCount) { line ->
                    val lineWidth = if (layout.lineCount == 1) layout.multiParagraph.intrinsics.maxIntrinsicWidth
                        else layout.getLineRight(line) - layout.getLineLeft(line)
                    assertTrue(lineWidth <= layout.size.width + 1.dp.value,
                        "$label line $line too wide: $lineWidth / ${layout.size.width}")
                    assertFalse(layout.isLineEllipsized(line), "$label must not ellipsize")
                }
                assertEquals(label.length, layout.getLineEnd(layout.lineCount - 1), "$label must lay out every character")
            }
            val outer = target.getUnclippedBoundsInRoot()
            val inner = text.getUnclippedBoundsInRoot()
            assertTrue(inner.left >= outer.left && inner.right <= outer.right && inner.top >= outer.top && inner.bottom <= outer.bottom,
                "$label text outside target: $inner / $outer")
            val visible = text.fetchSemanticsNode().boundsInRoot
            close(inner.width.value, visible.width, "$label visible text width")
            close(inner.height.value, visible.height, "$label visible text height")
            assertTouchTarget(outer, label)
        }
    }

    @Test fun headerRowsUseTheSameVerticalSpacing() = header {
        val contentTop = onNodeWithTag("shell-content").getUnclippedBoundsInRoot().top.value
        close(PorchlightTokens.spaceMd.toFloat(), bounds("Mine").top.value - contentTop, "Title bar / scope rhythm")
        val scopeBottom = options.maxOf { bounds(it).bottom.value }
        val actionTop = bounds("New item").top.value
        val actionBottom = bounds("New item").bottom.value
        val pending = onNode(hasText("DESIGN-PENDING:", substring = true) and hasText(screen, substring = true)).getUnclippedBoundsInRoot()
        close(PorchlightTokens.spaceSm.toFloat(), actionTop - scopeBottom, "Scope / action rhythm")
        close(PorchlightTokens.spaceSm.toFloat(), pending.top.value - actionBottom, "Action / content rhythm")
    }

    @Test fun renderTodayHeaderEvidence() {
        org.junit.Assume.assumeTrue(screen == "Today" && widthDp in listOf(360, 412))
        header {
            val phaseFile = evidenceDir.resolve("todayheader-phase.txt")
            val phase = if (phaseFile.exists()) phaseFile.readText().trim() else "after"
            require(phase in listOf("before", "after"))
            val pixels = onRoot().captureToImage().toPixelMap()
            val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
            for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
            evidenceDir.mkdirs()
            val file = evidenceDir.resolve("today-header-$phase-${widthDp}dp-${theme.name.lowercase()}-${fontScale.toInt()}x.png")
            assertTrue(ImageIO.write(image, "png", file))
        }
    }
}
