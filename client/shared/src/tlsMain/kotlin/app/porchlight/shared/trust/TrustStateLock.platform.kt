package app.porchlight.shared.trust

internal actual fun trustStateLock(): TrustStateLock = object : TrustStateLock {
    override fun <T> withLock(block: () -> T): T = synchronized(this, block)
}
