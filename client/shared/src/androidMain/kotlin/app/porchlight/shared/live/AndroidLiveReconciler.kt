package app.porchlight.shared.live

import app.porchlight.shared.data.CacheView
import app.porchlight.shared.data.CacheOwner
import app.porchlight.shared.data.owns
import app.porchlight.shared.trust.TrustController

/** One application-owned reconciliation path for collectors, permission recovery and background work. */
internal class AndroidLiveReconciler(
    private val controller: TrustController,
    private val view: () -> CacheView,
    private val alarms: (CacheView) -> Unit,
    private val delivery: (CacheView) -> Unit,
    private val reschedule: () -> Unit,
    private val clear: () -> Unit,
    private val beforeReconcile: suspend () -> Unit = {},
    private val loadOwner: () -> CacheOwner? = { null },
    private val saveOwner: (CacheOwner?) -> Unit = {},
) {
    private val identity = LiveIdentityFence(controller)
    suspend fun reconcile(recover: Boolean = false) {
        val epoch = identity.epoch()
        beforeReconcile()
        identity.run(epoch) {
            val current = view()
            if (current.sync != null) alarms(current)
            // Rebuilding cache preserves durable alarms; identity-end cancels them through clearIdentity.
            if (recover && identity.current(epoch)) reschedule()
            if (current.sync != null && identity.current(epoch)) delivery(current)
        }
    }
    fun epoch() = identity.epoch()
    suspend fun withCurrent(epoch: Long = identity.epoch(), action: (CacheView) -> Unit) = identity.run(epoch) { action(view()) }
    fun current() = identity.current(identity.epoch())
    suspend fun prepareIdentity(owner: CacheOwner, epoch: Long = controller.sessionEpoch.value) = identity.clear(epoch) {
        if (loadOwner()?.owns(owner) != true) clear()
        // Neither SQL reconciliation nor action-owner rotation consumes this retirement obligation.
        saveOwner(owner)
    }
    suspend fun clearIdentity(epoch: Long = controller.sessionEpoch.value) = identity.clear(epoch) { clear(); saveOwner(null) }
}
