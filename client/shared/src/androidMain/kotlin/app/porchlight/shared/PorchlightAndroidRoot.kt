package app.porchlight.shared

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import app.porchlight.shared.onboarding.AndroidPermissionOnboarding
import app.porchlight.shared.trust.*

/** Shared production wiring, also exercised by the debug-only deterministic fixture. */
@Composable
fun PorchlightAndroidRoot(
    services: AppServices,
    controller: TrustController,
    credentialGate: @Composable (() -> Unit) -> Unit = { AndroidPinGate(it) },
    onboarding: @Composable (content: @Composable () -> Unit) -> Unit = { AndroidPermissionOnboarding(services.settings, it) },
    scanner: (@Composable ((String) -> Unit) -> Unit)? = null,
) {
    TrustedPorchlightApp(services, controller,
        launchGate = { AndroidLaunchTrustGate(controller, credentialGate, it) },
        // This slot is reachable only after the launch gate checked DEVICE_CREDENTIAL.
        pinGate = { unlocked -> LaunchedEffect(Unit) { unlocked() } },
        onboarding = onboarding, scanner = scanner)
}
