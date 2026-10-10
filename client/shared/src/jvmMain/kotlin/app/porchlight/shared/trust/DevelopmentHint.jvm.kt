package app.porchlight.shared.trust

import androidx.compose.runtime.Composable

/** The 10.0.2.2 hint is Android-only. Desktop never advertises an unreachable emulator address. */
@Composable
internal actual fun platformDebugBuild(): Boolean = false
