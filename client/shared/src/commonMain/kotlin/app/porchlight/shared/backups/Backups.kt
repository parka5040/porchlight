package app.porchlight.shared.backups

import app.porchlight.shared.api.models.BackupStatus
import app.porchlight.shared.api.models.Bundle
import app.porchlight.shared.data.GeneratedCall
import app.porchlight.shared.data.HouseholdLocked
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.StateFlow
import kotlin.time.Instant

// The wire DTOs come from OpenAPI; only local presentation/stream headers live here.
data class DownloadHead(val status: Int, val etag: String?, val length: Long?, val contentRange: String?)
data class BackupView(val allowed: Boolean = false, val status: BackupStatus? = null, val localBundle: String? = null,
    val pulling: Boolean = false, val error: String? = null, val unreachableSince: Instant? = null,
    val notice: String? = null) {
    val cantReachHome get() = unreachableSince != null
}
interface DesktopBackups {
    val state: StateFlow<BackupView>
    suspend fun pull()
}
interface BackupSource {
    /** Reauthorize every attempt. Null means no backup request or disk mutation is allowed. */
    suspend fun authorizedHousehold(): String?
    fun ensureAuthorized() {}
    suspend fun status(): BackupStatus
    suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit)
}
class TrustedBackupSource(private val controller: TrustController) : BackupSource {
    private val call = GeneratedCall.trusted(controller)
    private var authorizedEpoch: Long? = null
    private var authorizedToken: String? = null
    private var superAdmin = false
    private fun authorized() {
        if (!superAdmin) throw HouseholdLocked()
        if (authorizedEpoch != controller.sessionEpoch.value || authorizedToken != controller.sessionToken()) throw CancellationException("Household session changed")
        if (controller.state.value != TrustState.Ready) throw HouseholdLocked()
    }
    override suspend fun authorizedHousehold(): String? {
        superAdmin = false
        if (controller.state.value != TrustState.Ready) return null
        authorizedEpoch = controller.sessionEpoch.value; authorizedToken = controller.sessionToken()
        val me = call.decode<app.porchlight.shared.api.models.MeResponse>(call.request("GET", "/v1/me", emptyMap(), emptyMap(), null))
        superAdmin = me.role == "super_admin"
        return me.householdId.takeIf { superAdmin }
    }
    override fun ensureAuthorized() = authorized()
    override suspend fun status(): BackupStatus {
        authorized()
        val status = call.decode<BackupStatus>(call.request("GET", "/v1/backups", emptyMap(), emptyMap(), null))
        authorized(); return status
    }
    override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
        val epoch = controller.sessionEpoch.value; val token = controller.sessionToken()
        fun fence() {
            authorized()
            if (epoch != controller.sessionEpoch.value || token != controller.sessionToken()) throw CancellationException("Household session changed")
            if (controller.state.value != TrustState.Ready) throw HouseholdLocked()
        }
        fence()
        try {
            controller.transport().download("/v1/backups/latest/download", token,
                if (offset > 0) "bytes=$offset-" else null, "\"${bundle.sha256}\"",
                { fence(); begin(it) }, { fence(); chunk(it) })
            fence()
        } catch (e: CancellationException) { throw e }
        catch (e: ApiProblem) { fence(); controller.onProblem(e); throw e }
        catch (e: DeviceCertificateRevoked) { fence(); controller.onTransportError(e); throw e }
    }
}
