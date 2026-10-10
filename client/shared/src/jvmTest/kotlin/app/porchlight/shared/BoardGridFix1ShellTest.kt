package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.*
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.*
import app.porchlight.shared.ui.TokenFittingText
import kotlin.test.*
import kotlinx.coroutines.launch

@OptIn(ExperimentalTestApi::class)
class BoardGridFix1ShellTest {
    @Test fun fix1R3TinyNavigationKeepsTouchTargetsAndLabels() {
        for (width in listOf(1, 160, 200, 240, 320)) for (scale in listOf(1f, 2f)) {
            runDesktopComposeUiTest(width = width, height = 800) {
                val services = FakeServices()
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, scale)) { PorchlightTheme(ThemeMode.Daylight) {
                    ShellNavigation(AppRoute.Today, DeviceSettings(), services.settings, {})
                } } }
                mainDestinations.forEach { (label, _) ->
                    val target = onNodeWithContentDescription("Navigate to $label").assertHasClickAction()
                    val bounds = target.getUnclippedBoundsInRoot()
                    assertTrue(bounds.width >= PorchlightTokens.touchTarget.dp, "$width/$scale: $label $bounds")
                    assertTrue(bounds.height >= PorchlightTokens.touchTarget.dp)
                    if (width < PorchlightTokens.touchTarget * mainDestinations.size) target.performScrollTo()
                    target.assertIsDisplayed()
                }
                if (width <= 240) onNodeWithTag("nav-label-Calendar", useUnmergedTree = true).assertDoesNotExist()
            }
        }
    }

    @Test fun fix1R1AllCollectionsKeepTheirWorkingItemAndCreationRoutes() {
        for (width in listOf(600, 1024)) for ((_, parent) in mainDestinations) {
            runDesktopComposeUiTest(width = width, height = 800) {
                lateinit var nav: androidx.navigation.NavHostController
                lateinit var scope: kotlinx.coroutines.CoroutineScope
                val services = FakeServices()
                setContent { PorchlightTheme(ThemeMode.Daylight) {
                    nav = androidx.navigation.compose.rememberNavController()
                    scope = rememberCoroutineScope()
                    ShellNavigation(parent, DeviceSettings(), services.settings, {}, nav = nav)
                } }
                waitForIdle()
                onNodeWithTag("detail-pane").assertDoesNotExist()
                for (detail in listOf(AppRoute.Item("fixture", "occ-fixture"), AppRoute.NewItem)) {
                    runOnIdle { scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) { nav.navigate(detail) } }
                    val label = if (detail is AppRoute.Item) "Item fixture" else "Item new"
                    onAllNodesWithText(label).assertCountEquals(1)
                    onNodeWithText(label).assertIsDisplayed()
                    if (width >= 840) onNodeWithTag("detail-pane").assertIsDisplayed()
                    else onNodeWithTag("detail-pane").assertDoesNotExist()
                    onNodeWithContentDescription("Go back").performClick()
                    onNodeWithTag("detail-pane").assertDoesNotExist()
                    onNodeWithText(label).assertDoesNotExist()
                }
            }
        }
    }

    @Test fun fix1R6FittingSelectionNeverWritesSnapshotStateDuringMeasure() = runDesktopComposeUiTest(width = 700, height = 600) {
        var available by mutableStateOf(600.dp)
        var writes = 0
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            Layout(modifier = Modifier.width(available), content = {
                Column {
                    TokenFittingText("Calendar", listOf(MaterialTheme.typography.headlineMedium, MaterialTheme.typography.labelSmall),
                        MaterialTheme.colorScheme.onSurface, textModifier = Modifier.padding(8.dp).testTag("fitting-label"))
                    ShellTitle("Calendar", true, {})
                }
            }) { items, constraints ->
                val child = Snapshot.observe(readObserver = null, writeObserver = { writes++ }) { items.single().measure(constraints) }
                layout(child.width, child.height) { child.placeRelative(0, 0) }
            }
        } }
        val actionId = onNodeWithContentDescription("Settings").fetchSemanticsNode().id
        val textId = onNodeWithTag("fitting-label", useUnmergedTree = true).fetchSemanticsNode().id
        for (width in listOf(600, 100, 480, 120, 600)) {
            runOnIdle { available = width.dp }
            onAllNodesWithContentDescription("Settings").assertCountEquals(1)
            onNodeWithContentDescription("Settings").assertHasClickAction().assertIsDisplayed()
            assertEquals(actionId, onNodeWithContentDescription("Settings").fetchSemanticsNode().id, "Action identity survives resizing")
            assertEquals(textId, onNodeWithTag("fitting-label", useUnmergedTree = true).assertIsDisplayed().fetchSemanticsNode().id)
        }
        assertEquals(0, writes, "Fitting must not mutate composition state from measurement")
    }
}
