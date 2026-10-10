package app.porchlight.shared.trust

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import androidx.compose.material3.MaterialTheme
import kotlin.test.Test
import kotlin.test.assertTrue

@OptIn(ExperimentalTestApi::class)
class TrustScreenTest {
    @Test fun enrollmentIsAccessibleAtDoubleFontScale() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { EnrollmentScreen(false, onEnroll = { _, _ -> }) } } }
        onNodeWithText("Enrollment URI").assertExists()
        onNodeWithText("Password").assertExists()
        onNodeWithText("Enroll device").performScrollTo().assertHasClickAction()
        val bounds = onNodeWithText("Enroll device").fetchSemanticsNode().boundsInRoot
        assertTrue(bounds.height >= 48f)
        onNodeWithText("ENROLL_HOST=10.0.2.2", substring = true).assertDoesNotExist()
    }
    @Test fun loginResetAndRecoveryHaveLabeledActions() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { LoginScreen(expired = true, onLogin = {}, onReset = {}) } } }
        onNodeWithText("Session expired").assertExists()
        onNodeWithText("Sign in").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Sign in").fetchSemanticsNode().boundsInRoot.height >= 48f)
        onNodeWithText("Use reset code").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Use reset code").fetchSemanticsNode().boundsInRoot.height >= 48f)
    }
    @Test fun resetAndUpgradeSemantics() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { ResetPasswordScreen(onReset = { _, _ -> }, onBack = {}) } } }
        onNodeWithText("10-character reset code").assertExists()
        onNodeWithText("New password").assertExists()
        onNodeWithText("Reset password").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Reset password").fetchSemanticsNode().boundsInRoot.height >= 48f)
        onNodeWithText("Back to sign in").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Back to sign in").fetchSemanticsNode().boundsInRoot.height >= 48f)
    }
    @Test fun gateNeverComposesRestoredContentBeforePin() = runComposeUiTest {
        val stored = DeviceTrust("device", "leaf", "root", "session", "2027-04-01T00:00:00Z", listOf("https://lan"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load() = stored
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = stored.certificateExpiresAt
        }
        val api = object : app.porchlight.shared.transport.ApiTransport {
            override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) =
                kotlinx.serialization.json.buildJsonObject {
                    if (path == "/v1/me") put("member_id", kotlinx.serialization.json.JsonPrimitive("member"))
                    else put("endpoints", kotlinx.serialization.json.JsonArray(listOf(kotlinx.serialization.json.JsonPrimitive("https://lan"))))
                }
        }
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }
        val controller = TrustController(keys, factory, "android", { kotlin.time.Instant.parse("2026-10-05T00:00:00Z") }, true)
        setContent { MaterialTheme { TrustGate(controller, android = true, pinGate = { unlocked ->
            TrustRecoveryScreen(TrustState.PinRequired, onRetry = unlocked)
        }) { androidx.compose.material3.Text("Restored private content") } } }
        onNodeWithText("Restored private content").assertDoesNotExist()
        onNodeWithText("Unlock with device credential").performScrollTo().performClick()
        onNodeWithText("Restored private content").assertExists()
    }
    @Test fun firstRestoreFailureShowsAndroidRecoveryAndClearsLocalTrust() = runComposeUiTest {
        var cleared = false
        val keys = object : UnusedKeys() {
            override suspend fun load(): DeviceTrust? = throw java.security.KeyStoreException("unrecoverable")
            override suspend fun clear() { cleared = true }
        }
        val controller = TrustController(keys, app.porchlight.shared.transport.OkHttpTrustFactory(keys,
            app.porchlight.shared.transport.ClientInfo("0.1.0")), "android", { kotlin.time.Clock.System.now() }, true)
        setContent { MaterialTheme { TrustGate(controller, android = true) { androidx.compose.material3.Text("Private content") } } }
        onNodeWithText("Private content").assertDoesNotExist()
        onNodeWithText("Android could not recover", substring = true).assertExists()
        onNodeWithText("Unlock your wallet").assertDoesNotExist()
        onNodeWithText("Clear trust and enroll again").performScrollTo().performClick()
        onNodeWithText("Enroll this device").assertExists()
        assertTrue(cleared)
    }
    @Test fun changePasswordAtDoubleScale() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            ChangePasswordScreen(onChange = { _, _ -> }, onBack = {})
        } } }
        onNodeWithText("Current password").assertExists()
        onNodeWithText("New password").assertExists()
        onNodeWithText("Save password").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Save password").fetchSemanticsNode().boundsInRoot.height >= 48f)
    }
    @Test fun stepUpHasAccessibleConfirmAtDoubleScale() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            StepUpDialog("device revocation", onConfirm = {}, onDismiss = {})
        } } }
        onNodeWithText("Password").assertExists()
        onNodeWithText("Confirm").assertHasClickAction()
        onNodeWithText("Cancel").assertHasClickAction()
        assertTrue(onNodeWithText("Confirm").fetchSemanticsNode().boundsInRoot.height >= 48f)
        assertTrue(onNodeWithText("Cancel").fetchSemanticsNode().boundsInRoot.height >= 48f)
    }
    @Test fun upgradeAndWalletRecoveryHaveLabeledActions() = runComposeUiTest {
        var state by androidx.compose.runtime.mutableStateOf<TrustState>(TrustState.UpgradeRequired("2.0.0"))
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            TrustRecoveryScreen(state) { state = TrustState.WalletLocked }
        } } }
        onNodeWithText("Upgrade required").assertExists()
        onNodeWithText("Check again").performScrollTo().performClick()
        onNodeWithText("Unlock your wallet").assertExists()
        onNodeWithText("Retry after unlock").performScrollTo().assertHasClickAction()
    }
    @Test fun pinAndReEnrollmentRecoveryAtDoubleScale() = runComposeUiTest {
        var state by androidx.compose.runtime.mutableStateOf<TrustState>(TrustState.PinRequired)
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            TrustRecoveryScreen(state) { state = TrustState.ReEnroll }
        } } }
        onNodeWithText("Unlock with device credential").performScrollTo().performClick()
        onNodeWithText("Re-enrollment required").assertExists()
        onNodeWithText("Enroll again").performScrollTo().assertHasClickAction()
    }
    // deskkey: Settings → Account and devices names the key's backend; the file backend carries the disk-encryption note.
    @Test fun ownDevicesShowWhichBackendProtectsTheKey() = runComposeUiTest {
        var protection by androidx.compose.runtime.mutableStateOf<KeyProtection?>(KeyProtection.OwnerOnlyFile)
        setContent { MaterialTheme { OwnDevicesScreen(emptyList(), onRevoke = {}, onLogout = {}, onChangePassword = {}, keyProtection = protection) } }
        onNodeWithText("owner-only key file", substring = true).assertExists()
        onNodeWithText("Protected by file permissions; keep your disk encrypted", substring = true).assertExists()
        protection = KeyProtection.SecretService
        onNodeWithText("Secret Service", substring = true).assertExists()
        onNodeWithText("keep your disk encrypted", substring = true).assertDoesNotExist()
        protection = null
        onNodeWithText("This device's key", substring = true).assertDoesNotExist()
    }
    // R85: the enrollment-time wallet screen offers the protected file only when the controller offers it.
    @Test fun enrollmentWalletScreenOffersTheProtectedFileOnlyWhenOffered() = runComposeUiTest {
        var chosen = false
        var offered by androidx.compose.runtime.mutableStateOf(true)
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            TrustRecoveryScreen(TrustState.WalletLocked, onFileKeyStore = if (offered) ({ chosen = true }) else null) {}
        } } }
        onNodeWithText("Retry after unlock").performScrollTo().assertHasClickAction()
        onNodeWithText("Protected by file permissions; keep your disk encrypted", substring = true).assertExists()
        onNodeWithText("Store the key in a protected file instead").performScrollTo().performClick()
        assertTrue(chosen)
        assertTrue(onNodeWithText("Store the key in a protected file instead").fetchSemanticsNode().boundsInRoot.height >= 48f)
        offered = false
        onNodeWithText("Store the key in a protected file instead").assertDoesNotExist()
        onNodeWithText("keep your disk encrypted", substring = true).assertDoesNotExist()
        onNodeWithText("Retry after unlock").assertExists()
    }
    @Test fun enrollmentScreenNamesAChosenKeyFile() = runComposeUiTest {
        setContent { MaterialTheme { EnrollmentScreen(false, onEnroll = { _, _ -> }, keyProtection = KeyProtection.OwnerOnlyFile) } }
        onNodeWithText("owner-only key file", substring = true).assertExists()
        onNodeWithText("Protected by file permissions; keep your disk encrypted", substring = true).assertExists()
    }
    @Test fun unusableKeyFolderNamesTheFolderAndTheFix() = runComposeUiTest {
        var retried = false
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) {
            TrustRecoveryScreen(TrustState.KeyStorageUnavailable("/home/me/.local/share/porchlight/device-key", "it is a symbolic link")) { retried = true }
        } } }
        onNodeWithText("/home/me/.local/share/porchlight/device-key", substring = true).assertExists()
        onNodeWithText("it is a symbolic link", substring = true).assertExists()
        onNodeWithText("won't help", substring = true).assertExists()
        onNodeWithText("Retry").performScrollTo().performClick()
        assertTrue(retried)
    }
    @Test fun ownDevicesAndStepUp() = runComposeUiTest {
        setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { OwnDevicesScreen(listOf(OwnDevice("device", "Kitchen tablet", "android", true, false)), onRevoke = {}, onLogout = {}, onChangePassword = {}) } } }
        onNodeWithText("Kitchen tablet").assertExists()
        onNodeWithContentDescription("Revoke Kitchen tablet").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithContentDescription("Revoke Kitchen tablet").fetchSemanticsNode().boundsInRoot.height >= 48f)
        onNodeWithText("Change password").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Change password").fetchSemanticsNode().boundsInRoot.height >= 48f)
        onNodeWithText("Log out").performScrollTo().assertHasClickAction()
        assertTrue(onNodeWithText("Log out").fetchSemanticsNode().boundsInRoot.height >= 48f)
    }
}
