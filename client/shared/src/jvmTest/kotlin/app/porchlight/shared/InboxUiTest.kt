package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.screens.inbox.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class InboxUiTest {
    @Test fun inboxAndBannerActionsAreAccessibleAtDoubleScaleBothThemes() {
        for (theme in listOf(ThemeMode.Evening, ThemeMode.Daylight)) runDesktopComposeUiTest(width=1000, height=1800) {
            var read: String? = null
            setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightTheme(theme, false) {
                InboxContent(listOf(InboxGroup("Today", listOf(notificationFixture()))), { read=it }, {}, {}, {})
            } } }
            for (label in listOf("Mark all read", "Notification preferences", "Got it: Mark New comment read")) {
                val n = onNodeWithContentDescription(label).assertExists().fetchSemanticsNode()
                assertTrue(n.boundsInRoot.width >= 48 && n.boundsInRoot.height >= 48)
            }
            onNodeWithContentDescription("Got it: Mark New comment read").performClick()
            assertEquals(notificationFixture().id, read)
            onNodeWithText("Comments · Unread").assertExists()
        }
        runDesktopComposeUiTest { setContent { PorchlightTheme(ThemeMode.Daylight, false) {
            ReachabilityBanner(Instant.parse("2026-10-06T12:00:00Z")) {}
        } }
            onNodeWithTag("unreachable-banner").assertExists()
            onNodeWithContentDescription("Retry connection").assertExists()
        }
    }
}
