package app.porchlight.shared

import androidx.compose.foundation.layout.*
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.ui.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class BoardGridTextTest {
    @Test fun fittingTextSupportsIntrinsicParents() = runDesktopComposeUiTest(width = 360, height = 500) {
        setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            Row(Modifier.height(IntrinsicSize.Min).width(IntrinsicSize.Min)) {
                TokenFittingText("Calendar", listOf(MaterialTheme.typography.labelMedium, MaterialTheme.typography.labelSmall), MaterialTheme.colorScheme.onSurface)
            }
        } }
        onNodeWithText("Calendar", useUnmergedTree = true).assertIsDisplayed()
    }
    @Test fun shellTitleSupportsIntrinsicParentsWithOneSettingsAction() = runDesktopComposeUiTest(width = 320, height = 500) {
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            Row(Modifier.height(IntrinsicSize.Min).width(IntrinsicSize.Min)) {
                app.porchlight.shared.navigation.ShellTitle("Calendar", true, {})
            }
        } } }
        onNodeWithText("Calendar", useUnmergedTree = true).assertIsDisplayed()
        onAllNodesWithContentDescription("Settings").assertCountEquals(1)
        val settings = onNodeWithContentDescription("Settings").assertIsDisplayed().assertHasClickAction()
        assertEquals(app.porchlight.shared.theme.PorchlightTokens.touchTarget.toFloat(), settings.getUnclippedBoundsInRoot().width.value,
            "Intrinsic minimum must select the gear, not a hidden duplicate text action")
    }
    @Test fun overwideSingleTokenNeverBreaksInsideWord() = runDesktopComposeUiTest(width = 320, height = 1000) {
        val label = "FixtureRepresentativeWithoutAnyWordSeparators"
        var selected by mutableStateOf("long")
        setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(ThemeMode.Daylight, false) {
            SegmentedChoice(listOf(ScopeOption("long", label), ScopeOption("short", "Other")), selected, { selected = it })
        } } }
        val results = mutableListOf<TextLayoutResult>()
        onNodeWithText(label, useUnmergedTree = true).performSemanticsAction(SemanticsActions.GetTextLayoutResult) { it(results) }
        assertEquals(1, results.single().lineCount, "A single over-wide token must scroll or ellipsize, never split")
        assertEquals(androidx.compose.ui.text.style.TextOverflow.Ellipsis, results.single().layoutInput.overflow)
        assertTrue(results.single().multiParagraph.intrinsics.maxIntrinsicWidth > results.single().size.width)
        onAllNodesWithText(label, useUnmergedTree = true).assertCountEquals(1)
        onNodeWithContentDescription(label).assertIsSelected().assertHasClickAction()
        onNodeWithContentDescription("Other").performClick().assertIsSelected()
        onNodeWithContentDescription(label).assertIsNotSelected()
    }
}
