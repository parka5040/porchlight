package app.porchlight.shared.live

import kotlinx.coroutines.withTimeoutOrNull

/** Preserve the opted-in healthy-stream safety net; stale or opted-out capable sessions replay instead. */
suspend fun backgroundSafetyNet(stayConnected: Boolean, connected: Boolean, capable: Boolean, cursor: Long?,
    live: LiveSync?, unread: suspend () -> Unit, refresh: suspend () -> Unit): Boolean =
    if (stayConnected && connected) liveSafetyNet(true, true, unread, refresh)
    else backgroundCatchUp(capable, cursor, live, refresh)

/** Legacy/missing-cache safety net remains a full read. Controls never become acknowledged cursors. */
suspend fun backgroundCatchUp(capable: Boolean, cursor: Long?, live: LiveSync?, refresh: suspend () -> Unit): Boolean =
    withTimeoutOrNull(60_000) {
        if (capable && cursor != null && cursor >= 0 && live != null) live.catchUp()
        else { refresh(); true }
    } ?: false
