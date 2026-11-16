package app.porchlight.shared

import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.screens.inbox.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiInboxTest {
    @Test fun everyInboxItemHasAnExplicitContentType() = runDesktopComposeUiTest(width = 800, height = 1000) {
        val state = LazyListState()
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            InboxContent(listOf(InboxGroup("Today", listOf(notificationFixture()))), {}, {}, {}, {}, state)
        } }
        waitForIdle()
        assertEquals(listOf("actions", "heading", "notification"), state.layoutInfo.visibleItemsInfo.map { it.contentType },
            "Every Inbox item must declare its reuse type")
    }
}
