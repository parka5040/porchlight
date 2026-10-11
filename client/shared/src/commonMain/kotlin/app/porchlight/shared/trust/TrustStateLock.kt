package app.porchlight.shared.trust

/** Short, reentrant state transitions only. Never hold across storage or transport calls. */
internal interface TrustStateLock {
    fun <T> withLock(block: () -> T): T
}
internal expect fun trustStateLock(): TrustStateLock
