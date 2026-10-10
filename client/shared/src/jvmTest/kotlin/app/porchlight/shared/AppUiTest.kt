package app.porchlight.shared

import androidx.compose.material3.Text
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.v2.runComposeUiTest
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.text.font.FontFamily
import app.porchlight.shared.resources.Res
import app.porchlight.shared.resources.literata_semibold
import org.jetbrains.compose.resources.Font
import kotlin.test.Test

/** Keeps Compose UI testing on the JVM target (and bundled font resources) compiling and running. */
@OptIn(ExperimentalTestApi::class)
class AppUiTest {
    @Test
    fun appShowsItsName() = runDesktopComposeUiTest(width = 1200, height = 900) {
        setContent { PorchlightApp(FakeServices()) }
        onNodeWithText("Porchlight").assertIsDisplayed()
    }

    @Test
    fun bundledFontRenders() = runComposeUiTest {
        setContent { Text("Today", fontFamily = FontFamily(Font(Res.font.literata_semibold))) }
        onNodeWithText("Today").assertIsDisplayed()
    }
}
