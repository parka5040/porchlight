package app.porchlight.android

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** API35/36 instrumented production-root wiring with debug-only memory services/credential seam. */
@RunWith(AndroidJUnit4::class)
class ShellInstrumentedTest {
    @get:Rule val compose = createAndroidComposeRule<ShellFixtureActivity>()

    @Test fun gatePrecedesShellAndRelocksOnBackground() {
        compose.onNodeWithText("Fixture device credential").assertIsDisplayed()
        compose.onNodeWithTag("shell-content").assertDoesNotExist()
        compose.onNodeWithContentDescription("Navigate to Board").assertDoesNotExist()
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.waitUntil(10_000) { compose.onAllNodesWithTag("shell-content").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithTag("shell-content").assertIsDisplayed()
        compose.onNodeWithContentDescription("Navigate to Board").performClick()
        compose.onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        compose.waitUntil { compose.activity.resumeRoute == app.porchlight.shared.navigation.AppRoute.Board() }
        val controller = compose.activity.controllerIdentity
        compose.activityRule.scenario.moveToState(androidx.lifecycle.Lifecycle.State.CREATED)
        compose.activityRule.scenario.moveToState(androidx.lifecycle.Lifecycle.State.RESUMED)
        compose.onNodeWithText("Fixture device credential").assertIsDisplayed()
        compose.onNodeWithTag("shell-content").assertDoesNotExist()
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.onNodeWithTag("shell-content").assertIsDisplayed()
        compose.onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        org.junit.Assert.assertEquals(app.porchlight.shared.navigation.AppRoute.Board(), compose.activity.resumeRoute)
        compose.activityRule.scenario.recreate()
        org.junit.Assert.assertSame(controller, compose.activity.controllerIdentity)
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        org.junit.Assert.assertEquals(app.porchlight.shared.navigation.AppRoute.Board(), compose.activity.resumeRoute)
    }

    @Test fun routeSurvivesActivityRecreationAndNavigationMeetsTouchMinimum() {
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.waitUntil(10_000) {
            compose.onAllNodesWithContentDescription("Navigate to Board").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithContentDescription("Navigate to Board").performClick()
        compose.onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        compose.activityRule.scenario.recreate()
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.waitUntil(10_000) {
            compose.onAllNodesWithContentDescription("Navigate to Board").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithContentDescription("Navigate to Board").assertIsSelected()
        listOf("Today", "Board", "Calendar", "Lists", "Inbox").forEach { label ->
            compose.onNodeWithContentDescription("Navigate to $label").assertIsDisplayed()
                .assertHeightIsAtLeast(48.dp).assertWidthIsAtLeast(48.dp)
        }
    }

    @Test fun edgeToEdgeKeepsContentAndControlsInsideSystemBarsWithoutPlatformActionBar() {
        compose.onNodeWithText("Fixture device credential").performClick()
        compose.waitUntil(10_000) {
            compose.onAllNodesWithTag("shell-safe-area").fetchSemanticsNodes().isNotEmpty()
        }
        val decor = compose.activity.window.decorView
        val bars = decor.rootWindowInsets.getInsets(
            android.view.WindowInsets.Type.systemBars() or android.view.WindowInsets.Type.displayCutout())
        val safe = compose.onNodeWithTag("shell-safe-area").fetchSemanticsNode().boundsInRoot
        org.junit.Assert.assertTrue(safe.top >= bars.top)
        org.junit.Assert.assertTrue(safe.bottom <= decor.height - bars.bottom)
        org.junit.Assert.assertTrue(safe.left >= bars.left)
        org.junit.Assert.assertTrue(safe.right <= decor.width - bars.right)
        val content = compose.onNodeWithTag("shell-content").fetchSemanticsNode().boundsInRoot
        org.junit.Assert.assertTrue(content.top >= safe.top && content.bottom <= safe.bottom)
        compose.onAllNodes(hasClickAction()).fetchSemanticsNodes().forEach {
            org.junit.Assert.assertTrue(it.boundsInRoot.top >= safe.top && it.boundsInRoot.bottom <= safe.bottom)
        }
        val theme = compose.activity.obtainStyledAttributes(intArrayOf(android.R.attr.windowActionBar))
        try { org.junit.Assert.assertFalse(theme.getBoolean(0, false)) } finally { theme.recycle() }
    }

}
