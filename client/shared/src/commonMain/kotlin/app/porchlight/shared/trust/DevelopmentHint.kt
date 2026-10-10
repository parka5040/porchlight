package app.porchlight.shared.trust

import androidx.compose.runtime.Composable

internal fun showDevelopmentHint(android: Boolean, debugBuild: Boolean) = android && debugBuild

@Composable
internal expect fun platformDebugBuild(): Boolean
