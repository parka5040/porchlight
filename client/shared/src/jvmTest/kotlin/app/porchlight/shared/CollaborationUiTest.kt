package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.lists.*
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class CollaborationUiTest {
    @Test fun listsExposeAccessibleMovesAndCheckInBothThemesAtDoubleFont() {
        for (theme in listOf(ThemeMode.Daylight, ThemeMode.Evening)) runDesktopComposeUiTest(width = 620, height = 2200) {
            var checked = false
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                ListContent(listFixture(), emptyList(), { _, _ -> checked = true }, {}, { _, _ -> }, {}, {})
            } } }
            onNodeWithContentDescription("Check a").performClick(); assertTrue(checked)
            val bounds = onNodeWithContentDescription("Move a down").fetchSemanticsNode().boundsInRoot
            assertTrue(bounds.height >= 48f && bounds.width >= 48f)
            onNodeWithContentDescription("Clear Got it").assertDoesNotExist()
        }
    }
    @Test fun occurrenceChecksToggleFromEntryAndClearRequiresManageNotCheck() = runDesktopComposeUiTest(width = 620, height = 1800) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        var requested: Boolean? = null
        val list = listFixture("one").let { row -> row.copy(propertyEntries = row.propertyEntries.map { it.copy(occurrenceId = row.occurrenceId, checkedAt = now, checkedBy = collabId("self")) }) }
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { ListContent(list, emptyList(), { _, checked -> requested = checked }, {}, { _, _ -> }, {}, {}) } }
        onNodeWithContentDescription("Uncheck a").performClick(); assertEquals(false, requested)
        onNodeWithContentDescription("Clear Got it").assertDoesNotExist()
    }
    @Test fun clearIsVisibleForManageWithoutCheck() = runDesktopComposeUiTest(width = 620, height = 1800) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val entry = entryFixture("a", "a0").copy(checkedAt = now, checkedBy = collabId("self"))
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { ListContent(listFixture().copy(capabilities = listOf("view", "manage"), propertyEntries = emptyList(), gotIt = listOf(entry)), emptyList(), { _, _ -> }, {}, { _, _ -> }, {}, {}) } }
        onNodeWithContentDescription("Clear Got it").assertExists()
        onNodeWithContentDescription("Uncheck a").assertDoesNotExist()
    }
    @Test fun longPressWithoutMovementDoesNotWriteReorder() = runDesktopComposeUiTest(width = 620, height = 1800) {
        var moves = 0
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { ListContent(listFixture(), emptyList(), { _, _ -> }, {}, { _, _ -> moves++ }, {}, {}) } }
        onNodeWithText("a").performTouchInput { longClick() }
        runOnIdle { assertEquals(0, moves) }
    }
    @Test fun commentWindowDeletedPlaceholderAndAttachmentTargets() = runDesktopComposeUiTest(width = 650, height = 1800) {
        val now = Instant.parse("2026-10-06T12:16:00Z")
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { Column {
            CommentRow(commentFixture(), emptyList(), collabId("self"), false, now, {}, {})
            CommentRow(commentFixture().copy(id = collabId("deleted"), deletedAt = now), emptyList(), collabId("self"), false, now, {}, {})
            AttachmentRow(attachmentFixture(), true, {}, {})
        } } }
        onNodeWithContentDescription("Edit comment").assertDoesNotExist()
        onNodeWithText("Comment deleted").assertExists()
        val bounds = onNodeWithContentDescription("Open notes.txt").fetchSemanticsNode().boundsInRoot
        assertTrue(bounds.height >= 48f && bounds.width >= 48f)
    }
}
