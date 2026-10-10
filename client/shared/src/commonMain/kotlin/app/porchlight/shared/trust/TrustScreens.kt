package app.porchlight.shared.trust

import app.porchlight.shared.transport.ApiProblem
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*

@Composable
internal fun TrustForm(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(app.porchlight.shared.theme.PorchlightTokens.spaceMd.dp), verticalArrangement = Arrangement.spacedBy(app.porchlight.shared.theme.PorchlightTokens.spaceMd.dp)) {
        Text(title, style = MaterialTheme.typography.headlineMedium)
        content()
    }
}
@Composable
internal fun TrustButton(label: String, enabled: Boolean = true, description: String? = null, onClick: () -> Unit) {
    Button(onClick, enabled = enabled, modifier = Modifier.fillMaxWidth().heightIn(min = app.porchlight.shared.theme.PorchlightTokens.touchTarget.dp).then(if (description == null) Modifier else Modifier.semantics { contentDescription = description })) { Text(label) }
}
@Composable
private fun PasswordField(label: String, value: String, changed: (String) -> Unit) {
    OutlinedTextField(value, changed, label = { Text(label) }, singleLine = true,
        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
        visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth().heightIn(min = app.porchlight.shared.theme.PorchlightTokens.touchTarget.dp))
}

// DESIGN-PENDING: enrollment (QR/paste)
@Composable
fun EnrollmentScreen(android: Boolean, busy: Boolean = false, onEnroll: (String, String) -> Unit, scanner: (@Composable ((String) -> Unit) -> Unit)? = null, onStartOver: () -> Unit = {},
                     keyProtection: KeyProtection? = null) {
    var uri by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    TrustForm("Enroll this device") {
        Text("At home, ask a household admin for a Porchlight enrollment QR code or URI. Your password and token are sent only after the household CA is verified.")
        keyProtection?.let { protection -> // desktop: a backend already chosen for this enrollment (R85)
            Text("This device's key will be kept in: ${protection.label}")
            protection.note?.let { Text(it) }
        }
        OutlinedTextField(uri, { uri = it }, label = { Text("Enrollment URI") }, modifier = Modifier.fillMaxWidth().heightIn(min = app.porchlight.shared.theme.PorchlightTokens.touchTarget.dp))
        if (android && scanner != null) TrustButton("Scan enrollment QR", !busy, "Scan enrollment QR code") { scanning = !scanning }
        if (scanning && scanner != null) scanner { uri = it; scanning = false }
        PasswordField("Password", password) { password = it }
        TrustButton("Enroll device", !busy) { onEnroll(uri, password); password = "" }
        Text("After a lost reply, re-enter the original URI and original password to recover it. Start over forgets only local pending enrollment and its unused key; it does not undo enrollment on the server. A new token may be needed.")
        TrustButton("Start over", !busy) { onStartOver(); uri = ""; password = ""; scanning = false }
        if (showDevelopmentHint(android, platformDebugBuild())) Text("Development emulator: set ENROLL_HOST=10.0.2.2 in the dev stack before requesting the enrollment URI. The token supplies the device label.")
    }
}

// DESIGN-PENDING: login + session expired
@Composable
fun LoginScreen(expired: Boolean = false, busy: Boolean = false, onLogin: (String) -> Unit, onReset: () -> Unit) {
    var password by remember { mutableStateOf("") }
    TrustForm(if (expired) "Session expired" else "Sign in") {
        Text(if (expired) "Sign in again on this enrolled device. Your local key remains safe." else "Enter your household password.")
        PasswordField("Password", password) { password = it }
        TrustButton("Sign in", !busy) { onLogin(password); password = "" }
        TrustButton("Use reset code", !busy, onClick = onReset)
    }
}

// DESIGN-PENDING: reset-code entry
@Composable
fun ResetPasswordScreen(busy: Boolean = false, onReset: (String, String) -> Unit, onBack: () -> Unit) {
    var code by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    TrustForm("Reset household password") {
        Text("Ask an admin for a reset code. Codes expire after 15 minutes; spaces, hyphens and lowercase are accepted.")
        OutlinedTextField(code, { code = it }, label = { Text("10-character reset code") }, modifier = Modifier.fillMaxWidth().heightIn(min = app.porchlight.shared.theme.PorchlightTokens.touchTarget.dp), singleLine = true)
        PasswordField("New password", password) { password = it }
        Text("Use at least 12 characters and avoid known breached passwords.")
        TrustButton("Reset password", !busy) { onReset(code, password); password = ""; code = "" }
        TrustButton("Back to sign in", !busy, onClick = onBack)
    }
}

// DESIGN-PENDING: step-up dialog
@Composable
fun StepUpDialog(action: String, busy: Boolean = false, onConfirm: (String) -> Unit, onDismiss: () -> Unit) {
    var password by remember { mutableStateOf("") }
    AlertDialog(onDismissRequest = { if (!busy) onDismiss() }, title = { Text("Confirm $action") },
        text = { Column(verticalArrangement = Arrangement.spacedBy(app.porchlight.shared.theme.PorchlightTokens.spaceMd.dp)) { Text("Re-enter your password for this action only."); PasswordField("Password", password) { password = it } } },
        confirmButton = { TrustButton("Confirm", !busy) { onConfirm(password); password = "" } },
        dismissButton = { TextButton(onDismiss, enabled = !busy, modifier = Modifier.heightIn(min = app.porchlight.shared.theme.PorchlightTokens.touchTarget.dp)) { Text("Cancel") } })
}

// DESIGN-PENDING: change password
@Composable
fun ChangePasswordScreen(busy: Boolean = false, onChange: (String, String) -> Unit, onBack: () -> Unit) {
    var current by remember { mutableStateOf("") }; var new by remember { mutableStateOf("") }
    TrustForm("Change password") {
        PasswordField("Current password", current) { current = it }
        PasswordField("New password", new) { new = it }
        Text("At least 12 characters. Other sessions will be signed out.")
        TrustButton("Save password", !busy) { onChange(current, new); current = ""; new = "" }
        TrustButton("Back to devices", !busy, onClick = onBack)
    }
}

data class OwnDevice(val id: String, val label: String, val platform: String, val current: Boolean, val revoked: Boolean)
// DESIGN-PENDING: own devices
@Composable
fun OwnDevicesScreen(devices: List<OwnDevice>, busy: Boolean = false, onRevoke: (OwnDevice) -> Unit, onLogout: () -> Unit, onChangePassword: () -> Unit,
                     keyProtection: KeyProtection? = null) {
    TrustForm("Your devices") {
        keyProtection?.let { protection ->
            // deskkey: which backend protects this device's key, chosen at enrollment.
            Text("This device's key: ${protection.label}")
            protection.note?.let { Text(it) }
        }
        for (device in devices) {
            Text(device.label, style = MaterialTheme.typography.titleLarge)
            Text("${device.platform}${if (device.current) " · This device" else ""}${if (device.revoked) " · Revoked" else ""}")
            if (!device.revoked) TrustButton("Revoke device", !busy, "Revoke ${device.label}") { onRevoke(device) }
        }
        if (devices.isEmpty()) Text("No devices loaded. Check your connection and retry.")
        TrustButton("Change password", !busy, onClick = onChangePassword)
        TrustButton("Log out", !busy, onClick = onLogout)
    }
}

// DESIGN-PENDING: PIN gate, upgrade required, re-enroll required, locked wallet
@Composable
fun TrustRecoveryScreen(state: TrustState, onClear: (() -> Unit)? = null, onFileKeyStore: (() -> Unit)? = null, updates: app.porchlight.shared.update.UpdateController? = null, onRetry: () -> Unit) {
    val (title, detail, button) = when (state) {
        is TrustState.UpgradeRequired -> Triple("Upgrade required", "Install Porchlight ${state.minVersion} or newer to continue.", "Check again")
        is TrustState.LocalTrustLost -> Triple("Local trust unavailable", if (state.android)
            "Android could not recover this device's Keystore credentials. Clear local trust and ask an admin for a new enrollment URI."
            else "The encrypted device credentials cannot be recovered. Clear local trust and ask an admin for a new enrollment URI.", "Clear trust and enroll again")
        TrustState.ReEnroll -> Triple("Re-enrollment required", "This device certificate has expired or was revoked. Ask an admin for a new enrollment URI.", "Enroll again")
        TrustState.WalletLocked -> if (onFileKeyStore != null) Triple("Unlock your wallet",
            "Secret Service is installed but can't be used to enroll this device: it did not start, it is locked, its unlock was dismissed or timed out, or it has no default keyring. Unlock it and retry, or store this device's key in a protected file instead.",
            "Retry after unlock")
            else Triple("Unlock your wallet", "This device's key is kept in Secret Service, which is locked or not running. Unlock your wallet (or start its provider), then retry. Porchlight never moves the key to another store; enrolling again is the only way to change where it is kept.", "Retry after unlock")
        is TrustState.KeyStorageUnavailable -> Triple("Key folder unusable", keyStorageProblem(state.path, state.reason), "Retry")
        TrustState.PinRequired -> Triple("Unlock Porchlight", "Use your Android device credential to continue. There is no password or biometric fallback.", "Unlock with device credential")
        else -> Triple("Loading trust", "Checking this device…", "Retry")
    }
    TrustForm(title) {
        Text(detail)
        if (state is TrustState.UpgradeRequired && updates != null) {
            app.porchlight.shared.update.UpdateStatus(updates, required = true)
            app.porchlight.shared.ui.DesignPending("Upgrade-required updater")
        }
        if (state is TrustState.LocalTrustLost && onClear != null) {
            TrustButton("Try again", onClick = onRetry)
            TrustButton(button, onClick = onClear)
        } else TrustButton(button, onClick = onRetry)
        if (state == TrustState.WalletLocked && onFileKeyStore != null) {
            // R85: an explicit choice at enrollment only, never a silent fallback.
            TrustButton("Store the key in a protected file instead", onClick = onFileKeyStore)
            KeyProtection.OwnerOnlyFile.note?.let { Text(it) }
        }
    }
}

/** Compose slot for App(gate=…). Defaults fail closed for a required PIN; C2 supplies AndroidPinGate. */
@Composable
fun TrustGate(controller: TrustController, android: Boolean = false,
              pinGate: @Composable (onUnlocked: () -> Unit) -> Unit = { TrustRecoveryScreen(TrustState.PinRequired) {} },
              onboarding: @Composable (content: @Composable () -> Unit) -> Unit = { it() },
              scanner: (@Composable ((String) -> Unit) -> Unit)? = null,
              updates: app.porchlight.shared.update.UpdateController? = null,
              content: @Composable () -> Unit) {
    val state by controller.state.collectAsState()
    val busy by controller.busy.collectAsState()
    val error by controller.actionError.collectAsState()
    val renewalWarning by controller.renewalWarning.collectAsState()
    val fileKeyStoreOffered by controller.fileKeyStoreOffered.collectAsState()
    var enrollmentKey by remember { mutableStateOf<KeyProtection?>(null) }
    LaunchedEffect(state) { if (state == TrustState.Enrollment) enrollmentKey = controller.keyProtection() }
    var reset by remember { mutableStateOf(false) }
    fun run(action: suspend () -> Unit) { controller.runAction(action) }
    LaunchedEffect(controller) { controller.ensureRestored() }
    LaunchedEffect(state, busy) { controller.refreshWhenReady() }
    Column(Modifier.fillMaxSize()) {
        error?.let { Text(it, Modifier.padding(16.dp)) }
        if (state == TrustState.Ready || state == TrustState.PinRequired)
            renewalWarning?.let { Text(it, Modifier.padding(16.dp)) }
        Box(Modifier.weight(1f)) {
            when (state) {
                TrustState.Enrollment -> EnrollmentScreen(android, busy, { uri, password -> run { controller.enroll(uri, password) } }, scanner, { run { controller.startOverEnrollment() } }, enrollmentKey)
                TrustState.Login, TrustState.SessionExpired -> if (reset) ResetPasswordScreen(busy, { code, pass -> run { controller.resetPassword(code, pass); reset = false } }, { reset = false })
                    else LoginScreen(state == TrustState.SessionExpired, busy, { pass -> run { controller.login(pass) } }, { reset = true })
                TrustState.PinRequired -> pinGate { controller.unlockPin() }
                TrustState.Ready -> onboarding(content)
                TrustState.ReEnroll -> TrustRecoveryScreen(state) { run { controller.startReEnrollment() } }
                is TrustState.LocalTrustLost -> TrustRecoveryScreen(state, onClear = { run { controller.startReEnrollment() } }) { run { controller.restore() } }
                TrustState.WalletLocked -> TrustRecoveryScreen(state, onFileKeyStore = if (fileKeyStoreOffered) ({ run { controller.storeKeyInFileForEnrollment() } }) else null) { run { controller.restore() } }
                is TrustState.UpgradeRequired -> TrustRecoveryScreen(state, updates = updates) { updates?.check(); run { controller.restore() } }
                is TrustState.KeyStorageUnavailable -> TrustRecoveryScreen(state) { run { controller.restore() } }
                TrustState.Loading -> Text("Checking device trust…", Modifier.padding(app.porchlight.shared.theme.PorchlightTokens.spaceMd.dp))
            }
        }
    }
}

/** C2 exposes this in Settings; not part of the navigation shell. */
@Composable
fun TrustAccountScreen(controller: TrustController) {
    val scope = rememberCoroutineScope()
    var devices by remember { mutableStateOf(emptyList<OwnDevice>()) }
    var target by remember { mutableStateOf<OwnDevice?>(null) }
    var changing by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var protection by remember { mutableStateOf<KeyProtection?>(null) }
    suspend fun refresh() { devices = controller.ownDevices() }
    fun run(action: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try { action() }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { error = "Action failed. Check your connection and retry." }
            finally { busy = false }
        }
    }
    LaunchedEffect(controller) { run { refresh() } }
    LaunchedEffect(controller) { protection = controller.keyProtection() }
    Column {
        error?.let { Text(it, Modifier.padding(16.dp)); TrustButton("Retry") { run { refresh() } } }
        if (changing) ChangePasswordScreen(busy, { old, new -> run { controller.changePassword(old, new); changing = false } }, { changing = false })
        else OwnDevicesScreen(devices, busy, { target = it }, { run { controller.logout() } }, { changing = true }, protection)
    }
    target?.let { device -> StepUpDialog("revoke ${device.label}", busy, { password -> run {
        val token = controller.reauth(password, "device.revoke")
        controller.revoke(device.id, token)
        target = null
        if (!device.current) refresh()
    } }, { target = null }) }
}
