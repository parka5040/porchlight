package app.porchlight.shared.trust

import android.content.Intent
import android.os.Bundle
import android.provider.Settings
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

private object CredentialResults {
    val callbacks = ConcurrentHashMap<String, (Boolean) -> Unit>()
}

/** Uses a separate FragmentActivity so C2 need not change MainActivity's base class. */
class TrustCredentialActivity : FragmentActivity() {
    private var completed = false
    private fun finishResult(success: Boolean) {
        if (completed) return
        completed = true
        intent.getStringExtra("request")?.let { CredentialResults.callbacks.remove(it)?.invoke(success) }
        finish()
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val request = intent.getStringExtra("request")
        if (request == null || !CredentialResults.callbacks.containsKey(request)) { finishResult(false); return }
        if (BiometricManager.from(this).canAuthenticate(BiometricManager.Authenticators.DEVICE_CREDENTIAL) != BiometricManager.BIOMETRIC_SUCCESS) { finishResult(false); return }
        val prompt = BiometricPrompt(this, ContextCompat.getMainExecutor(this), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { finishResult(true) }
            override fun onAuthenticationError(code: Int, message: CharSequence) { finishResult(false) }
        })
        prompt.authenticate(BiometricPrompt.PromptInfo.Builder().setTitle("Unlock Porchlight")
            .setSubtitle("Confirm your Android device credential")
            .setAllowedAuthenticators(BiometricManager.Authenticators.DEVICE_CREDENTIAL).build())
    }
}

// DESIGN-PENDING: credential-only PIN gate + no device credential recovery
@Composable
fun AndroidPinGate(onUnlocked: () -> Unit) {
    val context = LocalContext.current
    val latest by rememberUpdatedState(onUnlocked)
    val request = remember { UUID.randomUUID().toString() }
    var failed by remember { mutableStateOf(false) }
    fun prompt() {
        failed = false
        CredentialResults.callbacks[request] = { success -> if (success) latest() else failed = true }
        context.startActivity(Intent(context, TrustCredentialActivity::class.java).putExtra("request", request))
    }
    DisposableEffect(request) { onDispose { CredentialResults.callbacks.remove(request) } }
    LaunchedEffect(request) { prompt() }
    TrustForm("Unlock Porchlight") {
        androidx.compose.material3.Text(if (failed) "Device credential was cancelled or is unavailable. Set an Android PIN, pattern or password, then retry. No fallback is permitted." else "Confirm your device credential before restored content is shown.")
        TrustButton("Unlock with device credential") { prompt() }
        if (failed) TrustButton("Android lock settings") { context.startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS)) }
    }
}

/** Relock on backgrounding; platform actions and restored navigation remain behind TrustGate. */
@Composable
fun AndroidTrustLifecycle(controller: TrustController) {
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner, controller) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) controller.lockPin() }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer); controller.lockPin() }
    }
}

/** PIN comes before even enrollment/login. ON_STOP discards the entire trusted subtree. */
@Composable
fun AndroidLaunchTrustGate(
    controller: TrustController,
    credentialGate: @Composable (() -> Unit) -> Unit = { AndroidPinGate(it) },
    content: @Composable () -> Unit,
) {
    AndroidTrustLifecycle(controller)
    var unlocked by remember { mutableStateOf(false) }
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) unlocked = false
        }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    if (unlocked) content() else credentialGate { unlocked = true }
}
