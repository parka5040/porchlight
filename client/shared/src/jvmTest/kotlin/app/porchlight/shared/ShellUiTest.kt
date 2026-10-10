package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import app.porchlight.shared.ui.MemberChip
import app.porchlight.shared.theme.PorchlightTheme
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import androidx.navigation.NavDestination.Companion.hasRoute
import kotlin.test.*
import kotlin.time.Instant

class FakeServices(initial: DeviceSettings = DeviceSettings()) : AppServices {
    val state = MutableStateFlow(initial)
    override val settings = object : SettingsStore {
        override val settings = state
        override suspend fun saveRoute(route: AppRoute) { state.value = state.value.copy(currentRoute = route) }
        override suspend fun setTheme(mode: ThemeMode) { state.value = state.value.copy(themeMode = mode) }
        override suspend fun setStartScreen(screen: StartScreen) { state.value = state.value.copy(startScreen = screen) }
        override suspend fun setLimitedReminders(limited: Boolean) { state.value = state.value.copy(limitedReminders = limited) }
    }
    override val clock = AppClock { Instant.parse("2026-10-05T12:00:00Z") }
    override val platform = object : PlatformInfo {
        override val kind = PlatformKind.Desktop
        override val systemDark = MutableStateFlow(true)
    }
}

@OptIn(ExperimentalTestApi::class)
class ShellUiTest {
    @Test fun compactNavigationIsAccessibleAtDoubleFontScaleInBothThemes() {
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 360, height = 800) {
            setContent {
                CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
                    PorchlightApp(FakeServices(DeviceSettings(themeMode = mode)))
                }
            }
            waitUntil { onAllNodesWithContentDescription("Navigate to Today").fetchSemanticsNodes().isNotEmpty() }
            for ((label, _) in mainDestinations) {
                val node = onNodeWithContentDescription("Navigate to $label").assertIsDisplayed().fetchSemanticsNode()
                assertTrue(node.boundsInRoot.width >= 48f && node.boundsInRoot.height >= 48f, "$label ${node.boundsInRoot}")
                val labelNode = onNodeWithTag("nav-label-$label", useUnmergedTree = true).fetchSemanticsNode()
                assertTrue(node.boundsInRoot.contains(labelNode.boundsInRoot.topLeft) &&
                    node.boundsInRoot.contains(labelNode.boundsInRoot.bottomRight), "$label label clipped")
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                onNodeWithTag("nav-label-$label", useUnmergedTree = true).performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                assertTrue(layouts.isNotEmpty() && layouts.all { it.lineCount == 1 && !it.hasVisualOverflow && !it.isLineEllipsized(0) && it.multiParagraph.intrinsics.maxIntrinsicWidth <= it.size.width + 1f },
                    "$label must fit in full without ellipsis at this width")
            }
            onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach { node ->
                assertTrue(node.config.getOrNull(SemanticsProperties.ContentDescription)?.isNotEmpty() == true, "Missing description: $node")
                assertTrue(node.boundsInRoot.width >= 48f && node.boundsInRoot.height >= 48f)
            }
        }
    }
    @Test fun expandedNavigationIsAccessibleAtDoubleScaleInBothThemes() {
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width = 1200, height = 1000) {
            val services = FakeServices(DeviceSettings(themeMode = mode))
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightApp(services) } }
            waitUntil { onAllNodesWithContentDescription("Navigate to Today").fetchSemanticsNodes().isNotEmpty() }
            onNodeWithTag("detail-pane").assertDoesNotExist()
            onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach { node ->
                assertTrue(node.config.getOrNull(SemanticsProperties.ContentDescription)?.isNotEmpty() == true)
                assertTrue(node.boundsInRoot.width >= 48f && node.boundsInRoot.height >= 48f)
            }
            mainDestinations.forEach { (label, _) ->
                val layouts = mutableListOf<androidx.compose.ui.text.TextLayoutResult>()
                onNodeWithTag("nav-label-$label", useUnmergedTree = true).performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(layouts) }
                assertTrue(layouts.isNotEmpty() && layouts.all { it.lineCount == 1 && !it.hasVisualOverflow && !it.isLineEllipsized(0) && it.multiParagraph.intrinsics.maxIntrinsicWidth <= it.size.width + 1f },
                    "$label must fit in full without ellipsis at this width")
            }
        }
    }
    @Test fun expandedRailRestoresContextAndSettingsControlsWork() = runDesktopComposeUiTest(width = 1200, height = 1000) {
        val services = FakeServices(DeviceSettings(AppRoute.Calendar(CalendarView.Day, "2026-10-05")))
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithText("Mon, Oct 5, 2026").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithText("UTC").assertExists()
        onNodeWithTag("detail-pane").assertDoesNotExist()
        onNodeWithContentDescription("Settings").performClick()
        mainClock.advanceTimeBy(500)
        waitForIdle()
        onNodeWithContentDescription("Daylight: theme").assertIsDisplayed().performClick()
        waitUntil { services.state.value.themeMode == ThemeMode.Daylight }
        onNodeWithContentDescription("Board: start screen").performClick()
        waitUntil { services.state.value.startScreen == StartScreen.Board }
        waitForIdle()
    }
    @Test fun gateHidesRestoredContentUntilUnlocked() = runDesktopComposeUiTest {
        val unlocked = mutableStateOf(false)
        val fixture = FakeServices(DeviceSettings(AppRoute.Item("private")))
        val availableItem = object : AppServices by fixture { override suspend fun routeAvailable(route: AppRoute) = true }
        setContent { PorchlightApp(availableItem, gate = { content ->
            if (unlocked.value) content() else Text("Locked")
        }) }
        onNodeWithText("Locked").assertIsDisplayed()
        onNodeWithText("Item private").assertDoesNotExist()
        runOnIdle { unlocked.value = true }
        waitUntil { onAllNodesWithText("Item private").fetchSemanticsNodes().isNotEmpty() }
    }
    @Test fun navigationWritesContextAndBackNavigation() = runDesktopComposeUiTest(width = 360, height = 800) {
        val services = FakeServices()
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Navigate to Board").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Navigate to Board").performClick()
        waitUntil { services.state.value.currentRoute == AppRoute.Board() }
        onNodeWithContentDescription("Settings").performClick()
        waitUntil { services.state.value.currentRoute == AppRoute.Settings() }
        onNodeWithContentDescription("Go back").performClick()
        waitUntil { services.state.value.currentRoute == AppRoute.Board() }
    }
    @Test fun settingsControlsAreAccessibleAtDoubleScale() = runDesktopComposeUiTest(width = 360, height = 900) {
        val services = FakeServices(DeviceSettings(AppRoute.Settings()))
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightApp(services) } }
        waitUntil { onAllNodesWithContentDescription("Evening: theme").fetchSemanticsNodes().isNotEmpty() }
        val descriptions = ThemeMode.entries.map { "${if (it == ThemeMode.System) "Follow system" else it.name}: theme" } + StartScreen.entries.map { "${if (it == StartScreen.LastScreen) "Where I left off" else it.name}: start screen" } +
            listOf("Today: open settings", "Notifications: open settings", "Account and devices: open settings")
        descriptions.forEach { description ->
            val node = onNodeWithContentDescription(description).performScrollTo().assertIsDisplayed().fetchSemanticsNode()
            assertTrue(node.boundsInRoot.width >= 48f && node.boundsInRoot.height >= 48f, "$description ${node.boundsInRoot}")
        }
    }
    @Test fun settingsReadErrorCanBeRetriedWithoutExposingContent() = runDesktopComposeUiTest {
        val base = FakeServices()
        var fail = true
        val services = object : AppServices by base {
            override val settings = object : SettingsStore by base.settings {
                override val settings = kotlinx.coroutines.flow.flow<DeviceSettings> {
                    if (fail) throw java.io.IOException("unavailable")
                    emit(DeviceSettings())
                }
            }
        }
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Try again: retry loading").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Navigate to Today").assertDoesNotExist()
        runOnIdle { fail = false }
        onNodeWithContentDescription("Try again: retry loading").performClick()
        waitUntil { onAllNodesWithContentDescription("Navigate to Today").fetchSemanticsNodes().isNotEmpty() }
    }
    @Test fun settingsWriteErrorCanBeRetried() = runDesktopComposeUiTest {
        val base = FakeServices()
        var fail = true
        val services = object : AppServices by base {
            override val settings = object : SettingsStore by base.settings {
                override suspend fun saveRoute(route: AppRoute) {
                    if (fail) throw java.io.IOException("unavailable")
                    base.settings.saveRoute(route)
                }
            }
        }
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Try again: retry saving device settings").fetchSemanticsNodes().isNotEmpty() }
        runOnIdle { fail = false }
        onNodeWithContentDescription("Try again: retry saving device settings").performClick()
        waitUntil { onAllNodesWithContentDescription("Try again: retry saving device settings").fetchSemanticsNodes().isEmpty() }
    }
    @Test fun settingsNamesContainVisibleLabelsAndSelectionHasNonColorMarker() = runDesktopComposeUiTest(width = 600, height = 1000) {
        val services = FakeServices(DeviceSettings(AppRoute.Settings(), themeMode = ThemeMode.System))
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithText("Where I left off").fetchSemanticsNodes().isNotEmpty() }
        onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach { node ->
            val name = node.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString(" ").orEmpty()
            node.config.getOrNull(SemanticsProperties.Text)?.forEach { label ->
                assertTrue(name.contains(label.text, ignoreCase = true), "Label '${label.text}' absent from '$name'")
            }
        }
        onNodeWithTag("selection-Follow system", useUnmergedTree = true).assertExists()
        onNodeWithTag("selection-Where I left off", useUnmergedTree = true).assertExists()
        onNodeWithTag("selection-Evening", useUnmergedTree = true).assertDoesNotExist()
    }
    @Test fun safeDrawingInsetsPadRailBarsAndContent() {
        for (width in listOf(360, 1200)) runDesktopComposeUiTest(width = width, height = 900) {
            val services = FakeServices()
            setContent { PorchlightTheme(ThemeMode.Evening) {
                ShellNavigation(AppRoute.Today, DeviceSettings(), services.settings, {},
                    safeInsets = WindowInsets(left = 10, top = 24, right = 12, bottom = 32))
            } }
            waitUntil { onAllNodesWithTag("shell-content").fetchSemanticsNodes().isNotEmpty() }
            val safe = onNodeWithTag("shell-safe-area").fetchSemanticsNode().boundsInRoot
            assertEquals(10f, safe.left); assertEquals(24f, safe.top)
            assertEquals(width - 12f, safe.right); assertEquals(868f, safe.bottom)
            val content = onNodeWithTag("shell-content").fetchSemanticsNode().boundsInRoot
            assertTrue(content.top >= safe.top && content.bottom <= safe.bottom)
            onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach {
                assertTrue(it.boundsInRoot.top >= safe.top && it.boundsInRoot.bottom <= safe.bottom)
            }
        }
    }
    @Test fun topLevelSwitchesStayBoundedAndBackReturnsTodayThenExitsEvenAfterResume() {
        for (start in listOf(AppRoute.Today, AppRoute.Board("mine"))) runDesktopComposeUiTest(width = 360, height = 900) {
            val services = FakeServices()
            lateinit var nav: androidx.navigation.NavHostController
            lateinit var scope: kotlinx.coroutines.CoroutineScope
            setContent { PorchlightTheme(ThemeMode.Evening) {
                nav = androidx.navigation.compose.rememberNavController()
                scope = rememberCoroutineScope()
                ShellNavigation(start, DeviceSettings(), services.settings, {}, nav = nav)
            } }
            waitUntil { onAllNodesWithContentDescription("Navigate to Today").fetchSemanticsNodes().isNotEmpty() }
            repeat(3) {
                for (label in listOf("Board", "Today", "Calendar", "Lists", "Inbox")) {
                    onNodeWithContentDescription("Navigate to $label").performClick()
                    waitForIdle()
                    runOnIdle {
                        if (nav.currentDestination!!.hasRoute<AppRoute.Today>()) {
                            assertNull(nav.previousBackStackEntry, "Today must be the root of the tab stack")
                        } else {
                            assertTrue(nav.previousBackStackEntry?.destination?.hasRoute<AppRoute.Today>() == true,
                                "Every other tab must be directly above Today")
                        }
                    }
                }
            }
            var popped: Boolean? = null
            runOnIdle { scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) { popped = nav.popBackStack() } }
            waitUntil { popped != null }
            assertTrue(popped!!)
            waitForIdle()
            assertTrue(nav.currentDestination!!.hasRoute<AppRoute.Today>())
            runOnIdle { popped = null; scope.launch(kotlinx.coroutines.Dispatchers.Main.immediate) { popped = nav.popBackStack() } }
            waitUntil { popped != null }
            assertFalse(popped!!, "Today must allow activity back dispatcher to exit")
        }
    }
    @Test fun readFailureAfterLaunchKeepsCurrentRouteAndBackStack() = runDesktopComposeUiTest(width = 600, height = 1000) {
        val base = FakeServices()
        val fail = MutableStateFlow(false)
        val services = object : AppServices by base {
            override val settings = object : SettingsStore by base.settings {
                override val settings = kotlinx.coroutines.flow.combine(base.state, fail) { stored, failed ->
                    if (failed) throw java.io.IOException("unavailable")
                    stored
                }
            }
        }
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Navigate to Board").fetchSemanticsNodes().isNotEmpty() }
        onNodeWithContentDescription("Navigate to Board").performClick()
        waitUntil { base.state.value.currentRoute == AppRoute.Board() }
        onNodeWithContentDescription("Settings").performClick()
        waitUntil { base.state.value.currentRoute == AppRoute.Settings() }
        runOnIdle { fail.value = true }
        waitUntil { onAllNodesWithContentDescription("Try again: retry loading").fetchSemanticsNodes().isNotEmpty() }
        // The shell must remain composed, including the Settings -> Board history.
        onNodeWithContentDescription("Go back").assertExists()
        runOnIdle { fail.value = false }
        onNodeWithContentDescription("Try again: retry loading").performClick()
        waitUntil { onAllNodesWithContentDescription("Try again: retry loading").fetchSemanticsNodes().isEmpty() }
        onNodeWithContentDescription("Go back").performClick()
        waitUntil { base.state.value.currentRoute == AppRoute.Board() }
    }
    @Test fun bothThemesActuallyApplyTokensIncludingFixedRolesAndCardSurface() {
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest {
            lateinit var scheme: androidx.compose.material3.ColorScheme
            lateinit var palette: app.porchlight.shared.theme.PorchlightPalette
            setContent { PorchlightTheme(mode) {
                scheme = androidx.compose.material3.MaterialTheme.colorScheme
                palette = app.porchlight.shared.theme.LocalPorchlightPalette.current
                app.porchlight.shared.ui.PorchlightCard(Modifier.size(200.dp, 100.dp).testTag("token-card")) { }
            } }
            waitForIdle()
            val expected = if (mode == ThemeMode.Evening) app.porchlight.shared.theme.PorchlightTokens.Evening else app.porchlight.shared.theme.PorchlightTokens.Daylight
            assertEquals(expected.surface, onNodeWithTag("token-card").captureToImage().toPixelMap()[100, 50])
            assertEquals(expected.background, scheme.background)
            assertEquals(expected.surface, scheme.surface)
            assertEquals(expected, palette)
            assertEquals(expected.primary, scheme.primaryFixed)
            assertEquals(expected.onPrimary, scheme.onPrimaryFixedVariant)
            assertEquals(expected.secondary, scheme.secondaryFixedDim)
            assertEquals(expected.onSecondary, scheme.onSecondaryFixed)
            assertEquals(expected.info, scheme.tertiaryFixed)
            assertEquals(expected.onInfo, scheme.onTertiaryFixedVariant)
            assertEquals(expected.scrim, scheme.scrim)
            assertEquals(expected.onSurface, scheme.onSurfaceVariant)
        }
    }
    @Test fun tabStateRestoresContextAfterSwitchingAway() = runDesktopComposeUiTest(width = 600, height = 1000) {
        val services = FakeServices(DeviceSettings(AppRoute.Board("mine")))
        setContent { PorchlightApp(services) }
        waitUntil { services.state.value.currentRoute == AppRoute.Board("mine") }
        onNodeWithContentDescription("Navigate to Calendar").performClick()
        waitUntil { services.state.value.currentRoute is AppRoute.Calendar }
        onNodeWithContentDescription("Navigate to Board").performClick()
        waitUntil { services.state.value.currentRoute == AppRoute.Board("mine") }
    }
    @Test fun followSystemUpdatesLiveButManualThemeIgnoresPortalChanges() = runDesktopComposeUiTest(width = 360, height = 800) {
        val services = FakeServices(DeviceSettings(themeMode = ThemeMode.System))
        val system = services.platform.systemDark as MutableStateFlow<Boolean>
        setContent { PorchlightApp(services) }
        waitUntil { onAllNodesWithContentDescription("Navigate to Today").fetchSemanticsNodes().isNotEmpty() }
        fun background() = onNodeWithTag("app-background").captureToImage().toPixelMap()[0, 0]
        assertEquals(app.porchlight.shared.theme.PorchlightTokens.Evening.background, background())
        runOnIdle { system.value = false }
        waitForIdle()
        assertEquals(app.porchlight.shared.theme.PorchlightTokens.Daylight.background, background())
        runOnIdle { services.state.value = services.state.value.copy(themeMode = ThemeMode.Evening) }
        waitForIdle()
        assertEquals(app.porchlight.shared.theme.PorchlightTokens.Evening.background, background())
        runOnIdle { system.value = true }
        waitForIdle()
        runOnIdle { system.value = false }
        waitForIdle()
        assertEquals(app.porchlight.shared.theme.PorchlightTokens.Evening.background, background())
    }
    @Test fun memberChipRendersInitials() = runDesktopComposeUiTest {
        setContent { PorchlightTheme(ThemeMode.Evening) { MemberChip("Alex Lee", 2) } }
        onNodeWithText("AL", useUnmergedTree = true).assertIsDisplayed()
        onNodeWithContentDescription("Member Alex Lee").assertIsDisplayed()
    }
}
