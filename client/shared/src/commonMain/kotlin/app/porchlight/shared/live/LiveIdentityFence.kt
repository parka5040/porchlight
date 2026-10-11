package app.porchlight.shared.live

import app.porchlight.shared.trust.TrustController
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Runtime-local serialization only: OS work must never hold the cache/controller commit lock.
 * Admission starts closed. Restoration is proof only when the controller's entire cleanup has finished.
 * Each backlog unit rechecks [current]; a prepared effect is always owned by this runtime until withdrawn. */
internal class LiveIdentityFence(private val controller: TrustController?, private val mutex: Mutex = Mutex()) {
    private val admittedEpoch = MutableStateFlow<Long?>(null)
    private fun controllerEpoch() = controller?.sessionEpoch?.value ?: 0L
    private fun restoredEpoch(): Long? = if (controller == null) 0L else controller.liveAdmission.value
    fun epoch(): Long = admittedEpoch.value ?: restoredEpoch() ?: -1L
    fun current(expected: Long): Boolean = admittedEpoch.value == expected && controllerEpoch() == expected &&
        restoredEpoch() == expected
    suspend fun run(expected: Long = epoch(), action: suspend () -> Unit): Boolean = mutex.withLock {
        if (admittedEpoch.value == null) admittedEpoch.value = restoredEpoch()
        if (!current(expected)) return@withLock false
        action()
        true
    }
    suspend fun clear(expected: Long = controllerEpoch(), action: suspend () -> Unit) = mutex.withLock {
        if (expected != controllerEpoch()) return@withLock
        admittedEpoch.value = null
        action()
        admittedEpoch.value = expected
    }
}
