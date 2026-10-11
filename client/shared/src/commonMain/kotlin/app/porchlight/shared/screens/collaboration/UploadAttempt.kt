package app.porchlight.shared.screens.collaboration

import app.porchlight.shared.data.UploadProgress
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.job

/** Session-owned observation of the current attempt, including identical-request retries. No bytes are retained here. */
class UploadAttempt {
    private var job: Job? = null
    private val mutable = MutableStateFlow<UploadProgress?>(null)
    val progress = mutable.asStateFlow()
    suspend fun <T> track(operation: suspend ((Long, Long) -> Unit) -> T): T {
        val active = currentCoroutineContext().job
        job = active
        mutable.value = UploadProgress(0, 0)
        try { return operation { sent, total -> if (job === active) mutable.value = UploadProgress(sent, total) } }
        finally { if (job === active) { job = null; mutable.value = null } }
    }
    fun cancel() { job?.cancel() }
}
