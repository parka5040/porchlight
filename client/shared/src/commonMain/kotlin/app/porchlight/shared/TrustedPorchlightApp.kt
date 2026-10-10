package app.porchlight.shared

import androidx.compose.runtime.Composable
import app.porchlight.shared.trust.*

/** The production composition root. Neither restored navigation nor Account is composed before trust/onboarding. */
@Composable
fun TrustedPorchlightApp(
    services: AppServices,
    controller: TrustController,
    launchGate: @Composable (content: @Composable () -> Unit) -> Unit = { it() },
    pinGate: @Composable (onUnlocked: () -> Unit) -> Unit = { TrustRecoveryScreen(TrustState.PinRequired) {} },
    onboarding: @Composable (content: @Composable () -> Unit) -> Unit = { it() },
    scanner: (@Composable ((String) -> Unit) -> Unit)? = null,
) {
    // Initialize the application-owned cache/binding, without exposing it before the credential/trust gates.
    services.data
    PorchlightApp(services, gate = { shell ->
        launchGate {
            TrustGate(controller, android = services.platform.kind == PlatformKind.Android,
                pinGate = pinGate, onboarding = onboarding, scanner = scanner, updates = services.updates, content = shell)
        }
    }, accountContent = {
        androidx.compose.foundation.layout.Column {
            services.backups?.let { app.porchlight.shared.backups.BackupPanel(it) }
            TrustAccountScreen(controller)
        }
    })
}
