package app.porchlight.shared.update

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.*
import app.porchlight.shared.api.models.LatestRelease
import app.porchlight.shared.api.models.ServerRelease
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.settings.SettingsScreen
import app.porchlight.shared.settings.*
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class UpdateUiTest {
    private fun ComposeUiTest.evidence(name: String) {
        val pixels = onRoot().captureToImage().toPixelMap()
        val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
        for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
        val directory = File("build/test-evidence/updates").apply { mkdirs() }
        assertTrue(ImageIO.write(image, "png", File(directory, "$name.png")))
    }
    private fun services(scope: CoroutineScope, source: UpdateSource = TestUpdateSource()): AppServices {
        val store = UpdateSettings()
        val controller = UpdateController(source, TestUpdatePlatform(), store, scope, { Instant.fromEpochSeconds(0) }, 106, "0.1.6")
        runBlocking { controller.checkNow() }
        return object : AppServices by FakeServices() { override val updates = controller; override val settings = store }
    }
    @Test fun realShellBannerCanDismissThisVersionAt320AndDoubleScale() {
        for (width in listOf(320, 1200)) {
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            try { runDesktopComposeUiTest(width = width, height = 1000) {
                val services = services(scope)
                setContent { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { PorchlightApp(services) } }
                onNodeWithText("Porchlight 0.1.7 is available").assertExists()
                onNodeWithText("Update", substring = false).assertIsDisplayed().assertHasClickAction()
                evidence("shell-$width-2x")
                onNodeWithText("Later").performClick()
                waitUntil { onAllNodesWithText("Porchlight 0.1.7 is available").fetchSemanticsNodes().isEmpty() }
            } } finally { scope.cancel() }
        }
    }
    @Test fun realAboutAndAdminCardAreScrollableAndCopyableAt320DoubleScale() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val source = object : TestUpdateSource() {
            override suspend fun server() = ServerRelease("0.1.7", true, LatestRelease("0.1.8"), Instant.fromEpochSeconds(0), null, "/srv/data/porchlight-upgrade")
        }
        try { runDesktopComposeUiTest(width = 320, height = 1000) {
            val services = services(scope, source)
            setContent { PorchlightTheme(ThemeMode.Daylight, false) { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
                androidx.compose.material3.Surface(Modifier.fillMaxSize()) {
                    SettingsScreen(DeviceSettings(), SettingsSection.About, {}, {}, {}, services = services)
                }
            } } }
            onNodeWithText("App version:", substring = true).assertExists()
            onNodeWithText("Server version: 0.1.7").assertExists()
            onNodeWithText("Check for updates").performScrollTo().assertHasClickAction()
            evidence("about-320-2x")
            onNodeWithText("/srv/data/porchlight-upgrade 0.1.8").performScrollTo().assertIsDisplayed()
            evidence("admin-320-2x")
            onNodeWithText("Server 0.1.8 is available").assertExists()
        } } finally { scope.cancel() }
    }
    @Test fun real426ScreenRunsSameUpdateAndNoReleaseExplainsAdminAction() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val source = TestUpdateSource()
        try { runDesktopComposeUiTest(width = 320, height = 1000) {
            val controller = services(scope, source).updates!!
            setContent { PorchlightTheme(ThemeMode.Evening, false) { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
                androidx.compose.material3.Surface(Modifier.fillMaxSize()) {
                    TrustRecoveryScreen(TrustState.UpgradeRequired("0.1.7"), updates = controller) {}
                }
            } } }
            onNodeWithText("Update now").performScrollTo().assertHasClickAction()
            evidence("recovery-320-2x")
            onNodeWithText("Update now").performClick()
            waitUntil { onAllNodesWithText("Restart to finish").fetchSemanticsNodes().isNotEmpty() }
            controller.reset()
            source.failure = app.porchlight.shared.transport.ApiProblem("client-release-unavailable", 404, "Unavailable")
            runBlocking { controller.checkNow() }
            onNodeWithText("Ask your admin to upgrade the server").assertExists()
            onNodeWithText("Check again").performScrollTo().assertHasClickAction()
        } } finally { scope.cancel() }
    }
    @Test fun progressPermissionFailureAndRestartHaveUsableActions() = runDesktopComposeUiTest(width = 320, height = 1000) {
        var state by mutableStateOf<UpdateState>(UpdateState.Downloading(50, 100))
        setContent { PorchlightTheme(ThemeMode.Daylight, false) { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            androidx.compose.material3.Surface(Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) { UpdateStatusContent(state) }
            }
        } } }
        onNodeWithText("Downloading update: 50%").assertExists()
        onNodeWithText("Cancel update").assertHasClickAction()
        evidence("progress-320-2x")
        state = UpdateState.NeedsPermission
        onNodeWithText("Allow app updates").performScrollTo().assertHasClickAction()
        evidence("permission-320-2x")
        state = UpdateState.Failed(DOWNLOAD_MISMATCH)
        onNodeWithText(DOWNLOAD_MISMATCH).assertExists()
        onNodeWithText("Retry update").performScrollTo().assertHasClickAction()
        state = UpdateState.ReadyToRestart
        onNodeWithText("Restart to finish").assertHasClickAction()
    }
}
