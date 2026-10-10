package app.porchlight.shared

import app.porchlight.shared.live.DesktopSleepBus

/** Hermetic default for host tests: DesktopLive's production factory connects to the real logind system bus. */
internal object NoSleepBus : DesktopSleepBus {
    override fun watch(receive: (Boolean) -> Unit) {}
    override fun close() {}
}
