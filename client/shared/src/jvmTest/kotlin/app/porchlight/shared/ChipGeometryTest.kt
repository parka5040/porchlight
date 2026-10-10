package app.porchlight.shared

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.dp
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.DeviceSettings
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.ui.memberColor
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.screens.settings.ColorChoices
import app.porchlight.shared.screens.settings.memberColorChoices
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.ui.ShellButton
import kotlin.math.abs
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class ChipGeometryTest {
    @Test fun plainActionKeepsBaselineWidth() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 800) {
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) {
                            Column {
                                ShellButton("Mine", onClick = {})
                                TextButton({}, Modifier.heightIn(min = 48.dp).widthIn(min = 48.dp)
                                    .semantics { contentDescription = "Baseline Mine" }) {
                                    Text("Mine", style = MaterialTheme.typography.labelLarge)
                                }
                            }
                        }
                    }
                }
                val actual = onNodeWithContentDescription("Mine").fetchSemanticsNode().boundsInRoot
                val baseline = onNodeWithContentDescription("Baseline Mine").fetchSemanticsNode().boundsInRoot
                assertEquals(baseline.width, actual.width, "Plain action width ($theme, scale=$scale)")
                assertEquals(baseline.height, actual.height)
                if (scale == 1f) assertEquals(54f, actual.width)
            }
        }
    }

    @Test fun plainActionsDoNotExposeSelectionOrToggleState() = runDesktopComposeUiTest {
        setContent { PorchlightTheme(ThemeMode.Evening) { ShellButton("Save", onClick = {}) } }
        val config = onNodeWithContentDescription("Save").assertHasClickAction().fetchSemanticsNode().config
        assertFalse(config.contains(SemanticsProperties.Selected), "Plain action is not selectable")
        assertFalse(config.contains(SemanticsProperties.ToggleableState), "Plain action is not checkable")
        assertEquals(Role.Button, config[SemanticsProperties.Role])
    }

    @Test fun compactTopBarTitleRemainsVisibleAt360dp() {
        constrainedChipWrapsWithoutLosingItsIndicatorOrChangingBounds()
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 1200) {
                val services = FakeServices()
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) {
                            ShellNavigation(AppRoute.Settings(), DeviceSettings(), services.settings, {})
                        }
                    }
                }
                waitUntil { onAllNodesWithContentDescription("Go back").fetchSemanticsNodes().isNotEmpty() }
                val title = onAllNodesWithText("Settings").fetchSemanticsNodes()
                    .first { !it.config.contains(SemanticsActions.OnClick) && it.boundsInRoot.top < 100f }
                assertTrue(title.boundsInRoot.width >= 40f && title.boundsInRoot.height > 0f,
                    "Title disappeared: ${title.boundsInRoot} ($theme, scale=$scale)")
                assertTrue(title.boundsInRoot.left >= 0f && title.boundsInRoot.right <= 360f)
            }
        }
    }

    private fun constrainedChipWrapsWithoutLosingItsIndicatorOrChangingBounds() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 1000) {
                val selected = mutableStateOf(false)
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) {
                            ShellButton("Whole household", onClick = { selected.value = !selected.value },
                                modifier = Modifier.width(140.dp), selected = selected.value)
                        }
                    }
                }
                val before = onNodeWithContentDescription("Whole household").fetchSemanticsNode().boundsInRoot
                val label = onNodeWithText("Whole household", useUnmergedTree = true)
                assertTrue(label.fetchSemanticsNode().boundsInRoot.width >= 88f,
                    "Ordinary chip must not waste room on a symmetric trailing slot")
                val layouts = mutableListOf<TextLayoutResult>()
                label.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                assertTrue(layouts.single().lineCount > 1, "Constrained long label must wrap")
                assertFalse(layouts.single().didOverflowWidth)
                onNodeWithContentDescription("Whole household").performClick()
                val after = onNodeWithContentDescription("Whole household").assertIsSelected().fetchSemanticsNode().boundsInRoot
                assertEquals(before, after)
                val indicator = onNodeWithTag("selection-Whole household", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                assertTrue(indicator.left >= after.left && indicator.right <= after.right)
                assertTrue(after.width == 140f && after.height >= 48f)
            }
        }
    }

    @Test fun colorColumnIsOneRadioTargetIncludingTheDotAndCanBeBlocked() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 1200) {
                val selected = mutableStateOf("")
                val blocked = mutableStateOf(false)
                var writes = 0
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) { ColorChoices(selected.value, blocked.value) { selected.value = it; writes++ } }
                    }
                }
                memberColorChoices.forEach { (name, color) ->
                    val target = onNode(hasContentDescription(name) or hasContentDescription("Choose $name member color")).assertHasClickAction()
                    target.assert(SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.RadioButton))
                    val box = target.fetchSemanticsNode().boundsInRoot
                    val dot = onNodeWithTag("color-dot-$name", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                    assertTrue(box.height >= 48f && box.width >= 48f)
                    assertTrue(dot.top >= box.top && dot.bottom <= box.bottom)
                    target.performTouchInput { click(androidx.compose.ui.geometry.Offset(dot.center.x - box.left, dot.center.y - box.top)) }
                    target.assertIsSelected()
                    runOnIdle { assertEquals(color, selected.value) }
                }
                assertEquals(memberColorChoices.size, onAllNodes(hasClickAction(), useUnmergedTree = true).fetchSemanticsNodes().size,
                    "Each column has exactly one actionable semantics node")
                runOnIdle { blocked.value = true }
                val before = writes
                onNodeWithContentDescription("Yellow").assertIsNotEnabled().performClick()
                runOnIdle { assertEquals(before, writes) }
            }
        }
    }

    @Test fun onlySelectedColorHasAnOuterRingAndItsBoxNeverMoves() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 1200) {
                val selected = mutableStateOf("")
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) { ColorChoices(selected.value, false) { selected.value = it } }
                    }
                }
                val before = memberColorChoices.keys.associateWith {
                    onNodeWithTag("color-ring-box-$it", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                }
                memberColorChoices.forEach { (name, color) ->
                    runOnIdle { selected.value = color }
                    memberColorChoices.keys.forEach { other ->
                        val ring = onNodeWithTag("color-ring-$other", useUnmergedTree = true)
                        if (other == name) ring.assertExists() else ring.assertDoesNotExist()
                        assertEquals(before.getValue(other), onNodeWithTag("color-ring-box-$other", useUnmergedTree = true)
                            .fetchSemanticsNode().boundsInRoot)
                    }
                }
            }
        }
    }

    @Test fun colorRingContrastAndGapCoverEveryColorInBothThemes() {
        val helper = TokenContrastTest()
        fun hex(color: androidx.compose.ui.graphics.Color) = "#" + (color.toArgb() and 0xFFFFFF).toString(16).padStart(6, '0')
        fun contrast(a: androidx.compose.ui.graphics.Color, b: androidx.compose.ui.graphics.Color): Double {
            val x = helper.luminance(hex(a)); val y = helper.luminance(hex(b))
            return (maxOf(x, y) + .05) / (minOf(x, y) + .05)
        }
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            runDesktopComposeUiTest(width = 800, height = 600) {
                val selected = mutableStateOf("")
                var surface = androidx.compose.ui.graphics.Color.Unspecified
                setContent {
                    PorchlightTheme(theme) {
                        surface = LocalPorchlightPalette.current.surface
                        Box(Modifier.fillMaxSize().background(surface).padding(16.dp)) {
                            ColorChoices(selected.value, false) { selected.value = it }
                        }
                    }
                }
                memberColorChoices.forEach { (name, color) ->
                    runOnIdle { selected.value = color }
                    val dot = onNodeWithTag("color-dot-$name", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                    val pixels = onRoot().captureToImage().toPixelMap()
                    val y = dot.center.y.toInt()
                    val ring = pixels[dot.left.toInt() - 5, y]
                    val gap = pixels[dot.left.toInt() - 2, y]
                    assertTrue(contrast(ring, surface) >= 3.0, "$theme $name ring/surface: ${contrast(ring, surface)}")
                    assertEquals(hex(surface), hex(gap), "$theme $name must have a surface-coloured gap outside the dot")
                    assertEquals(hex(memberColor(color)), hex(pixels[dot.center.x.toInt(), y]), "$theme $name dot unchanged")
                    assertTrue(contrast(ring, gap) >= 3.0, "$theme $name outer ring must be separated from the dot by a visible gap")
                }
            }
        }
    }

    @Test fun chipBoundsNeverChangeWhenSelected() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 800, height = 1000) {
                val selected = mutableStateOf(false)
                val labels = listOf("Mine", "Household", "Yellow", "Whole household")
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) {
                            Column { labels.forEach { ShellButton(it, onClick = { selected.value = !selected.value }, selected = selected.value) } }
                        }
                    }
                }
                val before = labels.associateWith { onNodeWithContentDescription(it).assertIsNotSelected().fetchSemanticsNode().boundsInRoot }
                runOnIdle { selected.value = true }
                labels.forEach { label ->
                    val after = onNodeWithContentDescription(label).assertIsSelected().fetchSemanticsNode().boundsInRoot
                    assertEquals(before.getValue(label), after, "$label selected footprint ($theme, scale=$scale)")
                    assertTrue(after.width >= 48f && after.height >= 48f)
                    onNodeWithTag("selection-$label", useUnmergedTree = true).assertExists()
                }
                runOnIdle { selected.value = false }
                labels.forEach { assertEquals(before.getValue(it), onNodeWithContentDescription(it).fetchSemanticsNode().boundsInRoot) }
            }
        }
    }

    @Test fun everyColorDotIsCenteredOverItsLabelWithoutSelectionShift() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = 360, height = 1200) {
                val selected = mutableStateOf("")
                setContent {
                    CompositionLocalProvider(LocalDensity provides Density(1f, scale)) {
                        PorchlightTheme(theme) { ColorChoices(selected.value, false) { selected.value = it } }
                    }
                }
                fun bounds(name: String) = onNodeWithContentDescription(name).fetchSemanticsNode().boundsInRoot
                val before = memberColorChoices.keys.associateWith { bounds(it) }
                fun checkColumns() {
                    val widths = memberColorChoices.map { (name, color) ->
                        val button = onNodeWithContentDescription(name)
                        if (selected.value == color) button.assertIsSelected() else button.assertIsNotSelected()
                        val dot = onNodeWithTag("color-dot-$name", useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                        val label = onNodeWithText(name, useUnmergedTree = true).fetchSemanticsNode().boundsInRoot
                        assertTrue(abs(dot.center.x - label.center.x) <= 1f, "$name dot=$dot label=$label ($theme, scale=$scale)")
                        assertTrue(dot.bottom <= label.top, "$name dot must be above label")
                        val box = bounds(name)
                        assertTrue(box.width >= 48f && box.height >= 48f)
                        assertEquals(before.getValue(name), box, "$name selection shifted column")
                        box.width
                    }
                    assertTrue(widths.max() - widths.min() <= 1f, "Column widths: $widths")
                }
                checkColumns()
                memberColorChoices.forEach { (name, _) ->
                    onNodeWithContentDescription(name).performClick()
                    checkColumns()
                }
            }
        }
    }
}
