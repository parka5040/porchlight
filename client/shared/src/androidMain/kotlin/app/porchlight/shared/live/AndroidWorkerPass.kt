package app.porchlight.shared.live

/** The unconstrained local pass cannot invoke the network callback, including on retry/offline starts. */
internal suspend fun householdWorkerPass(network: Boolean, reconcile: suspend () -> Unit,
    synchronize: suspend () -> Boolean): Boolean {
    reconcile()
    if (!network) return true
    val completed = synchronize()
    if (completed) reconcile()
    return completed
}
