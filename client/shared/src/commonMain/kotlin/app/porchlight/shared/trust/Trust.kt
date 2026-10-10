package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import app.porchlight.shared.transport.dto.DeviceList
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.channels.Channel
import kotlin.time.Instant

sealed interface TrustState {
    data object Loading : TrustState
    data object Enrollment : TrustState
    data object Login : TrustState
    data object SessionExpired : TrustState
    data object PinRequired : TrustState
    data object Ready : TrustState
    data object ReEnroll : TrustState
    data class LocalTrustLost(val android: Boolean) : TrustState
    data class UpgradeRequired(val minVersion: String) : TrustState
    data object WalletLocked : TrustState
    /** Desktop: the owner-only folder holding this device's key record/secret is unsafe (symlink, another user's,
     * no POSIX permissions). A persistent local condition: fixed by the user, never by clearing trust. */
    data class KeyStorageUnavailable(val path: String, val reason: String) : TrustState
}
/** Server answers that prove a STORED attempt can never succeed; anything else may precede an earlier committed send. */
internal val definitiveEnrollmentRejections = setOf("enrollment-token-invalid", "enrollment-token-used", "enrollment-token-expired",
    "invalid-credentials", "account-locked", "unauthenticated", "csr-invalid")
fun stateForProblem(p: ApiProblem): TrustState? = when {
    p.status == 426 && p.type == "upgrade-required" -> TrustState.UpgradeRequired(p.extras["min_client_version"]?.jsonPrimitive?.contentOrNull ?: "unknown")
    p.status == 401 && p.type in setOf("session-invalid", "session-device-mismatch", "unauthenticated") -> TrustState.SessionExpired
    p.status == 401 && p.type in setOf("device-revoked", "device-unknown", "device-certificate-expired") -> TrustState.ReEnroll
    else -> null
}
fun terminalRetrySuccess(action: String, p: ApiProblem) = p.status == 401 && when (action) {
    "logout" -> p.type in setOf("session-invalid", "unauthenticated")
    "self-revoke" -> p.type in setOf("device-revoked", "device-unknown", "session-invalid")
    else -> false
}
fun needsRenewal(now: Instant, expires: Instant) = expires - now < 30.days
fun normalizeResetCode(input: String): String? {
    val normalized = buildString {
        for (c in input) {
            if (c == '-' || c == ' ') continue
            append(when (c.uppercaseChar()) { 'O' -> '0'; 'I', 'L' -> '1'; else -> c.uppercaseChar() })
        }
    }
    return normalized.takeIf { it.length == 10 && it.all { c -> c in "0123456789ABCDEFGHJKMNPQRSTVWXYZ" } }
}

/** Deliberately redacts all credentials in diagnostics; persisted only through platform encrypted stores. */
@Serializable
class DeviceTrust(val deviceId: String, val certificate: String, val caCertificate: String, val session: String?,
                  val sessionExpiresAt: String?, val endpoints: List<String>, val certificateExpiresAt: String, val memberId: String? = null) {
    override fun toString() = "DeviceTrust(…redacted)"
    fun withSession(token: String?, expires: String?) = DeviceTrust(deviceId, certificate, caCertificate, token, expires, endpoints, certificateExpiresAt, memberId)
    fun withCertificate(pem: String, expires: String) = DeviceTrust(deviceId, pem, caCertificate, session, sessionExpiresAt, endpoints, expires, memberId)
    fun withIdentity(member: String, urls: List<String>) = DeviceTrust(deviceId, certificate, caCertificate, session, sessionExpiresAt, urls, certificateExpiresAt, member)
}
class LocalTrustUnavailable : Exception("Local trust cannot be recovered; re-enroll this device")
/** This device's key secret is in Secret Service, which is locked or unreachable. Never answered by another store. */
open class WalletUnavailable(cause: Throwable? = null) : Exception("Unlock your Secret Service wallet, then retry. This device's key stays in Secret Service; there is no fallback store.", cause)
/** R85: while enrolling a device that has no key yet, Secret Service was present but unusable (failed to start, no
 * default collection, unlock dismissed or timed out). The gate may offer the owner-only file as an explicit choice. */
class SecretServiceUnusableForEnrollment(cause: Throwable?) : WalletUnavailable(cause)
/** Which store protects this device's key secret (desktop; owner decision 2026-10-07). Fixed at enrollment. */
enum class KeyProtection(val id: String, val label: String, val note: String?) {
    SecretService("secret-service", "Secret Service (your desktop wallet)", null),
    OwnerOnlyFile("file", "owner-only key file", "Protected by file permissions; keep your disk encrypted"),
}
/** Persistent and user-fixable: names the folder and why; retrying alone does not help. */
fun keyStorageProblem(path: String, reason: String) = "This device's key is kept in $path, which is unusable because $reason. " +
    "It must be a folder you own that only you can read (not a link). Fix it, then retry; retrying alone won't help. " +
    "Deleting it means enrolling this device again."
/** No URI, token, password, password verifier or private key is serialized. The target binding
 * includes the high-entropy token; metadata itself stays in encrypted secure storage. */
@Serializable
class EnrollmentAttempt(val targetBinding: String, val idempotencyKey: String,
    val csr: String, val keyAlias: String) {
    override fun toString() = "EnrollmentAttempt(…redacted)"
}
/** Preserve ambiguous attempts for other tokens too; changing URI is not proof of rejection. */
@Serializable
class PendingEnrollments(val attempts: List<EnrollmentAttempt> = emptyList()) {
    fun find(binding: String?) = if (binding == null) attempts.lastOrNull() else attempts.find { it.targetBinding == binding }
    fun with(attempt: EnrollmentAttempt) = PendingEnrollments(attempts.filterNot { it.targetBinding == attempt.targetBinding } + attempt)
    fun without(binding: String?) = PendingEnrollments(if (binding == null) emptyList() else attempts.filterNot { it.targetBinding == binding })
    override fun toString() = "PendingEnrollments(…redacted)"
}
internal expect fun enrollmentBinding(input: String): String
internal expect fun newTrustRequestKey(): String

interface DeviceKeyStore {
    val enrollmentKeyAlias: String get() = "device"
    suspend fun loadEnrollmentAttempt(targetBinding: String? = null): EnrollmentAttempt? = null
    /** Must commit durably before any enrollment send. No in-memory/plaintext fallback. */
    suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) { error("Durable enrollment storage unavailable") }
    suspend fun clearEnrollmentAttempt(targetBinding: String? = null) { }
    /** Local only: drop ALL pending attempts and the unused enrollment key, never enrolled trust. */
    suspend fun abandonEnrollment() { clearEnrollmentAttempt() }
    suspend fun load(): DeviceTrust?
    suspend fun csr(): String
    /** Validate trust; atomically commit encrypted replacement AND remove any pending enrollment. */
    suspend fun save(trust: DeviceTrust, expectedPin: String? = null)
    suspend fun clear()
    fun certificateExpiry(pem: String): String
    /** Desktop: the backend recorded at enrollment (non-secret; never unlocks anything). Null where not applicable. */
    suspend fun keyProtection(): KeyProtection? = null
    /** R85: record the owner-only file for a device that has no key yet. Refuses once a key exists (re-enroll). */
    suspend fun chooseFileKeyStore() { throw UnsupportedOperationException("No alternative key store on this platform") }
}
interface TrustTransportFactory {
    fun enrollment(uri: EnrollUri): ApiTransport
    suspend fun authenticated(trust: DeviceTrust): ApiTransport
}

/** One coordinator per device. C2 injects gate/QR/platform permissions without changing app navigation. */
class TrustController(
    private val keys: DeviceKeyStore,
    private val factory: TrustTransportFactory,
    private val platform: String,
    private val now: () -> Instant,
    private val pinRequired: Boolean,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
) {
    private val mutex = Mutex()
    private val actionScope = CoroutineScope(SupervisorJob() + dispatcher)
    private val closed = MutableStateFlow(false)
    private val restored = MutableStateFlow(false)
    private val refreshStarted = MutableStateFlow(false)
    private val renewalWake = Channel<Unit>(Channel.CONFLATED)
    private val mutableBusy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = mutableBusy
    private val mutableActionError = MutableStateFlow<String?>(null)
    val actionError: StateFlow<String?> = mutableActionError
    private val mutableRenewalWarning = MutableStateFlow<String?>(null)
    val renewalWarning: StateFlow<String?> = mutableRenewalWarning
    private val refreshMutex = Mutex()
    private val mutableFileKeyStoreOffered = MutableStateFlow(false)
    /** R85: true only after an enrollment failed because Secret Service was present but unusable for a device with
     * no key yet. The wallet screen then also offers the owner-only file; restore/Retry withdraws the offer. */
    val fileKeyStoreOffered: StateFlow<Boolean> = mutableFileKeyStoreOffered

    /** UI consumers may disappear on rotation; trust mutations belong to this controller. */
    fun runAction(action: suspend () -> Unit): Job? {
        if (closed.value || !mutableBusy.compareAndSet(false, true)) return null
        mutableActionError.value = null
        return actionScope.launch {
            try { action() }
            catch (e: CancellationException) { throw e }
            catch (e: app.porchlight.shared.files.PrivateDirectoryUnavailable) { mutableActionError.value = keyStorageProblem(e.path, e.reason) }
            catch (e: ApiProblem) { mutableActionError.value = if (e.type == "idempotency-key-reused") "This enrollment was already processed. Enter the original password, or choose Start over." else if (e.type == "upgrade-required") "This app version is too old. Update Porchlight, then retry the enrollment with the same password." else "Request failed: ${e.type}. Try again or ask a household admin." }
            catch (_: Exception) { mutableActionError.value = "Unable to complete this action. Check your connection or wallet and retry." }
            finally { mutableBusy.value = false }
        }
    }
    fun ensureRestored() {
        if (restored.compareAndSet(false, true)) runAction { restore() }
    }
    /** Wake a wall-clock deadline after suspend; never reset an existing retry deadline/backoff. */
    fun onForeground() { renewalWake.trySend(Unit) }
    /** Application-lifetime obligation, not a busy UI action. R64 permits work while PIN-locked. */
    fun refreshWhenReady() {
        if (closed.value || !refreshStarted.compareAndSet(false, true)) return
        actionScope.launch(start = CoroutineStart.UNDISPATCHED) {
            var retryMillis = 1.minutes.inWholeMilliseconds
            var nextAt = now()
            while (isActive) {
                state.first { it == TrustState.Ready || it == TrustState.PinRequired }
                val remaining = (nextAt - now()).inWholeMilliseconds
                if (remaining > 0) {
                    withTimeoutOrNull(remaining.coerceAtMost(1.days.inWholeMilliseconds)) { renewalWake.receive() }
                    continue // foreground/unlock rechecks wall time, but cannot skip a retry deadline
                }
                while (renewalWake.tryReceive().isSuccess) { /* consume old foreground signals */ }
                val wait = try {
                    val notDue = refreshEndpointsAndRenewInternal(false)
                    retryMillis = 1.minutes.inWholeMilliseconds
                    val saved = mutex.withLock { trust }
                    if (notDue || saved == null) 1.days.inWholeMilliseconds
                    else {
                        val untilThreshold = Instant.parse(saved.certificateExpiresAt) - now() - 30.days
                        // Configurable short lifetimes must not turn a new certificate into a retry storm.
                        if (untilThreshold.inWholeMilliseconds < 0) 1.days.inWholeMilliseconds
                        else (untilThreshold.inWholeMilliseconds + 1).coerceIn(1L, 1.days.inWholeMilliseconds)
                    }
                } catch (e: CancellationException) { throw e }
                catch (_: Exception) {
                    mutex.withLock {
                        trust?.let {
                            if (trustState in setOf(TrustState.Ready, TrustState.PinRequired) &&
                                Instant.parse(it.certificateExpiresAt) - now() <= 7.days)
                                mutableRenewalWarning.value = "Device certificate expires soon. Renewal has not succeeded; reconnect or ask a household admin before it expires."
                        }
                    }
                    val backoff = retryMillis
                    retryMillis = (retryMillis * 2).coerceAtMost(60.minutes.inWholeMilliseconds)
                    backoff
                }
                nextAt = now() + wait.milliseconds
            }
        }
    }
    private val mutableState = MutableStateFlow<TrustState>(TrustState.Loading)
    val state: StateFlow<TrustState> = mutableState
    private val epoch = MutableStateFlow(0L)
    /** Household identity epoch: advances only when the identity that owns cached household data ends or changes
     * (logout, re-enrollment/revocation, a different device or member). Restore, close, PIN relock, wallet/upgrade
     * gates and session expiry followed by a same-identity login keep it, so the persisted read cache survives. */
    val sessionEpoch: StateFlow<Long> = epoch
    // Cache commits and epoch advances share a linearization point. Never held across network calls.
    internal val sessionCommitMutex = Mutex()
    /** Set by the data binding: awaited durable household-cache clear. A successful clear removes rows before
     * logout returns. Clear failures are swallowed; the epoch-change retry is best-effort, not a guarantee. */
    internal var identityEnded: (suspend () -> Unit)? = null
    private suspend fun endIdentity() {
        sessionCommitMutex.withLock { epoch.update { it + 1 } }
        try { identityEnded?.invoke() }
        catch (e: CancellationException) { currentCoroutineContext().ensureActive() } // e.g. cache closed
        catch (_: Exception) { /* the binding clears again on the epoch change */ }
    }
    private fun identityChanged(previous: DeviceTrust, next: DeviceTrust) = previous.deviceId != next.deviceId ||
        (previous.memberId != null && next.memberId != null && previous.memberId != next.memberId)
    private var trustState: TrustState
        get() = mutableState.value
        set(value) {
            val previous = mutableState.value
            mutableState.value = value
            // PIN relock/unlock is UI-only: do not reset a pending timer/backoff.
            if (value != previous && !(value in setOf(TrustState.Ready, TrustState.PinRequired) &&
                        previous in setOf(TrustState.Ready, TrustState.PinRequired))) renewalWake.trySend(Unit)
        }
    /** Enrollment/ReEnroll mean no live identity owns the household data any more. */
    private suspend fun transition(value: TrustState) {
        if (value != mutableState.value && (value == TrustState.Enrollment || value == TrustState.ReEnroll)) endIdentity()
        trustState = value
    }
    private val revision = MutableStateFlow(0L)
    /** Reconnect on a token replacement even if StateFlow conflates expiry and same-identity login. Not an identity epoch. */
    internal val sessionRevision: StateFlow<Long> = revision
    private var trust: DeviceTrust? = null
        set(value) { if (field?.session != value?.session) revision.update { it + 1 }; field = value }
    private var api: ApiTransport? = null
    private var connectedTrust: DeviceTrust? = null
    private var pinUnlocked = !pinRequired
    private suspend fun store(value: DeviceTrust) {
        keys.save(value)
        trust?.let { if (identityChanged(it, value)) endIdentity() }
        trust = value
    }
    private suspend fun connect(value: DeviceTrust) {
        api?.close()
        api = null
        connectedTrust = null
        val connected = factory.authenticated(value)
        if (closed.value) {
            connected.close()
            throw CancellationException("Trust controller closed")
        }
        api = connected
        connectedTrust = value
    }
    private suspend fun connectIfChanged(value: DeviceTrust) {
        val current = connectedTrust
        if (api == null || current == null || current.certificate != value.certificate ||
            current.caCertificate != value.caCertificate || current.endpoints != value.endpoints) connect(value)
    }
    private fun signedIn() {
        trustState = if (pinUnlocked) TrustState.Ready else TrustState.PinRequired
    }
    suspend fun restore() = mutex.withLock {
        check(!closed.value) { "Trust controller closed" }
        val previous = trust
        api?.close()
        api = null
        trust = null
        pinUnlocked = !pinRequired
        mutableFileKeyStoreOffered.value = false
        try {
            val saved = keys.load()
            if (previous != null && saved != null && identityChanged(previous, saved)) endIdentity()
            trust = saved
            if (saved == null) {
                transition(TrustState.Enrollment)
                return@withLock
            }
            if (Instant.parse(saved.certificateExpiresAt) <= now()) {
                transition(TrustState.ReEnroll)
                return@withLock
            }
            connect(saved)
            if (saved.session == null) trustState = TrustState.Login
            else if (saved.sessionExpiresAt == null || Instant.parse(saved.sessionExpiresAt) <= now()) trustState = TrustState.SessionExpired
            else signedIn()
        } catch (e: CancellationException) {
            throw e
        } catch (_: WalletUnavailable) {
            trustState = if (platform == "desktop") TrustState.WalletLocked else TrustState.LocalTrustLost(true)
        } catch (e: app.porchlight.shared.files.PrivateDirectoryUnavailable) {
            // Not lost trust: the key is intact behind a folder the user can fix. Never offer to clear it.
            trustState = TrustState.KeyStorageUnavailable(e.path, e.reason)
        } catch (_: Exception) {
            api?.close()
            api = null
            trust = null
            trustState = TrustState.LocalTrustLost(platform == "android")
        }
    }
    /** Closing is not an identity change: the persisted cache stays for the next process. */
    fun close() {
        if (!closed.compareAndSet(false, true)) return
        // Serialize disposal with connect; a factory completing after close also sees closed above.
        actionScope.launch {
            mutex.withLock { api?.close(); api = null }
            actionScope.cancel()
        }
    }
    fun unlockPin() {
        pinUnlocked = true
        if (trustState == TrustState.PinRequired) signedIn()
        onForeground()
    }
    fun lockPin() {
        if (!pinRequired) return
        pinUnlocked = false
        if (trustState == TrustState.Ready) signedIn()
    }
    private suspend fun call(method: String, path: String, body: JsonObject? = null, session: Boolean = true, stepUp: String? = null, key: String? = null): JsonObject {
        if (trustState == TrustState.PinRequired && (path.startsWith("/v1/devices") || path == "/v1/me" ||
            path == "/v1/endpoints" || path in setOf("/v1/auth/password", "/v1/auth/reauth") || stepUp != null))
            throw app.porchlight.shared.data.HouseholdLocked()
        try { return checkNotNull(api).request(method, path, body, if (session) trust?.session else null, key, stepUp = stepUp) }
        catch (e: DeviceCertificateRevoked) {
            onTransportError(e)
            throw e
        }
        catch (p: ApiProblem) {
            stateForProblem(p)?.let { state ->
                transition(state)
                if (state == TrustState.SessionExpired) trust?.let { store(it.withSession(null, null)) }
            }
            throw p
        }
    }
    private val enrollmentRetryAt = mutableMapOf<String, Instant>()
    private val enrollmentBackoff = mutableMapOf<String, Long>()
    private suspend fun clearAttemptSafely(target: String) {
        // Cleanup must never replace a definitive server/transport problem with a storage error.
        try { keys.clearEnrollmentAttempt(target) }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { /* Start over can retry local cleanup. */ }
        enrollmentRetryAt.remove(target)
        enrollmentBackoff.remove(target)
    }
    /** R85: the user's explicit choice after [fileKeyStoreOffered]; records the owner-only file exactly like
     * PORCHLIGHT_KEY_STORE=file, then returns to enrollment. Never available to an enrolled device. */
    suspend fun storeKeyInFileForEnrollment() = mutex.withLock {
        check(mutableFileKeyStoreOffered.value && trustState == TrustState.WalletLocked) { "The key file is offered only at enrollment" }
        keys.chooseFileKeyStore()
        mutableFileKeyStoreOffered.value = false
        transition(TrustState.Enrollment)
    }
    suspend fun startOverEnrollment() = mutex.withLock {
        keys.abandonEnrollment()
        enrollmentRetryAt.clear()
        enrollmentBackoff.clear()
    }
    suspend fun enroll(text: String, password: String) = mutex.withLock {
        val uri = EnrollUri.parse(text)
        EnrollmentTarget.parse(uri.host)
        val target = enrollmentBinding(jsonBody("host" to uri.host, "ca" to uri.caHex,
            "token" to uri.token, "platform" to platform).toString())
        var created = false // only an attempt THIS call created is provably unsent on a connect failure
        val attempt = try {
            val saved = keys.loadEnrollmentAttempt(target)
            if (saved != null) {
                check(saved.keyAlias == keys.enrollmentKeyAlias) { "Enrollment key unavailable" }
                saved
            } else {
                EnrollmentAttempt(target, newTrustRequestKey(), keys.csr(), keys.enrollmentKeyAlias)
                    .also { keys.saveEnrollmentAttempt(it); created = true }
            }
        } catch (e: WalletUnavailable) {
            trustState = TrustState.WalletLocked
            mutableFileKeyStoreOffered.value = e is SecretServiceUnusableForEnrollment
            throw e
        }
        enrollmentRetryAt[target]?.let { deadline ->
            val remaining = (deadline - now()).inWholeMilliseconds
            if (remaining > 0) delay(remaining.coerceAtMost(60_000))
        }
        val enrollment = try { factory.enrollment(uri) }
            catch (e: Exception) { if (created) clearAttemptSafely(target); throw e } // no request could have left
        var committed = false
        try {
            // No label: server deny_unknown_fields; label comes from the enrollment token.
            val response = enrollment.request("POST", "/v1/enrollment/complete", jsonBody(
                "token" to uri.token,
                "password" to password,
                "csr" to attempt.csr,
                "platform" to platform), idempotencyKey = attempt.idempotencyKey)
            val session = response.getValue("session").jsonObject
            val certificate = response.getValue("certificate").jsonPrimitive.content
            val value = DeviceTrust(
                response.getValue("device_id").jsonPrimitive.content,
                certificate,
                response.getValue("ca_certificate").jsonPrimitive.content,
                session.getValue("token").jsonPrimitive.content,
                session.getValue("expires_at").jsonPrimitive.content,
                orderedEndpoints(response.getValue("endpoints").jsonArray.map { it.jsonPrimitive.content }),
                keys.certificateExpiry(certificate))
            keys.save(value, uri.caHex)
            trust?.let { if (identityChanged(it, value)) endIdentity() }
            trust = value
            committed = true
            connect(value)
            signedIn()
        } catch (e: WalletUnavailable) {
            trustState = TrustState.WalletLocked
            throw e
        } catch (p: ApiProblem) {
            if (p.type == "idempotency-key-reused") {
                // Proves only that the first request was processed, not that it failed: keep the attempt.
                throw ApiProblem(p.type, p.status, "Enter the original password or Start over", p.detail, p.extras, p.retryAfterMillis)
            } else if (p.status in 400..499 && p.type != "idempotency-in-progress" && (created || p.type in definitiveEnrollmentRejections)) clearAttemptSafely(target)
            else {
                // Pre-processing rejections (429/408/426...) of a stored attempt say nothing about an earlier send: keep it.
                val wait = (p.retryAfterMillis ?: enrollmentBackoff[target] ?: 1000L).coerceIn(1000L, 60_000L)
                enrollmentRetryAt[target] = now() + wait.milliseconds
                enrollmentBackoff[target] = (wait * 2).coerceAtMost(60_000L)
            }
            throw p
        } catch (e: CancellationException) {
            throw e
        }
        catch (e: Exception) {
            // An unknown server outcome or unsaved success keeps the attempt for exact replay.
            // Only an attempt created by this call can be discarded on a provably-unsent failure.
            if (created && (e is ConnectFailure || e is RequestNotSent || (e is DeviceCertificateRevoked && e.requestNotSent))) clearAttemptSafely(target)
            if (committed) trustState = TrustState.LocalTrustLost(platform == "android")
            throw e
        } finally {
            enrollment.close()
        }
    }
    private suspend fun acceptSession(response: JsonObject) {
        val session = response.getValue("session").jsonObject
        store(checkNotNull(trust).withSession(session.getValue("token").jsonPrimitive.content, session.getValue("expires_at").jsonPrimitive.content))
        signedIn()
    }
    suspend fun login(password: String) = mutex.withLock { acceptSession(call("POST", "/v1/auth/login", jsonBody("password" to password), session = false)) }
    suspend fun resetPassword(code: String, newPassword: String) = mutex.withLock {
        val normalized = normalizeResetCode(code) ?: throw IllegalArgumentException("Enter the 10-character reset code")
        acceptSession(call("POST", "/v1/auth/reset-password", jsonBody("code" to normalized, "new_password" to newPassword), session = false))
    }
    suspend fun changePassword(current: String, new: String) = mutex.withLock {
        call("POST", "/v1/auth/password", jsonBody("current_password" to current, "new_password" to new))
        Unit
    }
    suspend fun reauth(password: String, action: String): String = mutex.withLock {
        call("POST", "/v1/auth/reauth", jsonBody("password" to password, "action" to action)).getValue("step_up_token").jsonPrimitive.content
    }
    suspend fun ownDevices(): List<OwnDevice> = mutex.withLock {
        val me = call("GET", "/v1/me")
        val member = me.getValue("member_id").jsonPrimitive.content
        val response = call("GET", "/v1/devices")
        val devices = try { WireJson.decodeFromJsonElement<DeviceList>(response).devices }
            catch (_: Exception) { throw ApiProblem("invalid-response", 502, "Invalid device list response") }
        devices.filter { it.memberId == member }.map { OwnDevice(it.id, it.label, it.platform, it.current, it.revokedAt != null) }
    }
    suspend fun revoke(deviceId: String, stepUp: String, key: String? = null) = mutex.withLock {
        val self = deviceId == trust?.deviceId
        try { call("POST", "/v1/devices/$deviceId/revoke", buildJsonObject {}, stepUp = stepUp, key = key) }
        catch (e: DeviceCertificateRevoked) { if (!self) throw e }
        catch (p: ApiProblem) { if (!self || !terminalRetrySuccess("self-revoke", p)) throw p }
        if (self) {
            keys.clear()
            trust = null
            api?.close()
            api = null
            transition(TrustState.ReEnroll)
        }
    }
    suspend fun logout(key: String? = null) = mutex.withLock {
        try { call("POST", "/v1/auth/logout", buildJsonObject {}, key = key) }
        catch (p: ApiProblem) { if (!terminalRetrySuccess("logout", p)) throw p }
        trust?.let { store(it.withSession(null, null)) }
        endIdentity() // Logout ends the identity's claim on cached data, even if the same member signs in again.
        trustState = TrustState.Login
    }
    /** Public clients use the 30-day threshold; server renewal-too-early overrides that heuristic. */
    suspend fun refreshEndpointsAndRenew() { refreshEndpointsAndRenewInternal(false) }
    internal suspend fun refreshEndpointsAndRenew(forceRenewal: Boolean) { refreshEndpointsAndRenewInternal(forceRenewal) }
    private data class RefreshSnapshot(val value: DeviceTrust, val transport: ApiTransport, val epoch: Long)
    private fun isCurrent(snapshot: RefreshSnapshot) = trust === snapshot.value && api === snapshot.transport &&
        epoch.value == snapshot.epoch && trustState in setOf(TrustState.Ready, TrustState.PinRequired)
    private suspend fun refreshSnapshot(): RefreshSnapshot? = mutex.withLock {
        if (closed.value || trustState !in setOf(TrustState.Ready, TrustState.PinRequired)) return@withLock null
        val value = checkNotNull(trust)
        connectIfChanged(value) // also retry a failed post-save rebuild
        RefreshSnapshot(value, checkNotNull(api), epoch.value)
    }
    private suspend fun refreshCall(snapshot: RefreshSnapshot, method: String, path: String, body: JsonObject? = null): JsonObject {
        // Neither the UI busy flag nor the controller mutex spans the network's 300-second read timeout.
        try { return snapshot.transport.request(method, path, body, snapshot.value.session) }
        catch (e: DeviceCertificateRevoked) {
            mutex.withLock { if (isCurrent(snapshot)) transition(TrustState.ReEnroll) }
            throw e
        } catch (p: ApiProblem) {
            mutex.withLock {
                if (isCurrent(snapshot)) stateForProblem(p)?.let {
                    transition(it)
                    if (it == TrustState.SessionExpired) store(snapshot.value.withSession(null, null))
                }
            }
            throw p
        }
    }
    /** Serializes refreshes with each other only. Late responses are fenced from logout/re-enrollment/login. */
    private suspend fun refreshEndpointsAndRenewInternal(forceRenewal: Boolean): Boolean = refreshMutex.withLock refresh@ {
        val initial = refreshSnapshot() ?: return@refresh true
        val me = refreshCall(initial, "GET", "/v1/me")
        val endpoints = orderedEndpoints(refreshCall(initial, "GET", "/v1/endpoints").getValue("endpoints").jsonArray.map { it.jsonPrimitive.content })
        val snapshot = mutex.withLock commitEndpoints@ {
            if (!isCurrent(initial)) return@commitEndpoints null
            store(initial.value.withIdentity(me.getValue("member_id").jsonPrimitive.content, endpoints)
                .withSession(initial.value.session, me["session_expires_at"]?.jsonPrimitive?.contentOrNull ?: initial.value.sessionExpiresAt))
            connectIfChanged(checkNotNull(trust))
            RefreshSnapshot(checkNotNull(trust), checkNotNull(api), epoch.value)
        } ?: return@refresh true
        if (forceRenewal || needsRenewal(now(), Instant.parse(snapshot.value.certificateExpiresAt))) {
            val csr = keys.csr()
            if (!mutex.withLock { isCurrent(snapshot) }) return@refresh true
            val renewal = try { refreshCall(snapshot, "POST", "/v1/devices/self/certificate", jsonBody("csr" to csr)) }
            catch (p: ApiProblem) {
                if (p.status != 409 || p.type != "renewal-too-early" || forceRenewal) throw p
                // Internal forceRenewal probes intentionally expose the wire problem; clients never force.
                mutableRenewalWarning.value = null
                return@refresh true // daily not-due cadence, not a failure/backoff escalation
            }
            mutex.withLock commitCertificate@ {
                if (!isCurrent(snapshot)) return@commitCertificate
                val certificate = renewal.getValue("certificate").jsonPrimitive.content
                store(snapshot.value.withCertificate(certificate, keys.certificateExpiry(certificate)))
                connectIfChanged(checkNotNull(trust))
            }
        }
        mutableRenewalWarning.value = null
        false
    }
    /** C2/C4 use this for their generated transport, and report problems and typed transport errors back to this gate. */
    suspend fun onProblem(problem: ApiProblem) { stateForProblem(problem)?.let { transition(it) } }
    suspend fun onTransportError(error: DeviceCertificateRevoked) { transition(TrustState.ReEnroll) }
    fun transport(): ApiTransport = checkNotNull(api)
    /** A PIN relock hides UI content, not the stored session/key. All other trust gates remain terminal for background work. */
    fun backgroundSessionUsable(): Boolean = !closed.value && trust?.session != null &&
        state.value in setOf(TrustState.Ready, TrustState.PinRequired)
    /** Stable within an enrolled identity, including session expiry/re-login; no session secrets. */
    // Member discovery (null -> known) is not identity-end; real member/device changes clear owners in endIdentity.
    fun notificationIdentity(): String? = trust?.deviceId
    /** Settings → Account and devices: which backend protects this device's key (display only; failures read as unknown). */
    suspend fun keyProtection(): KeyProtection? = try { keys.keyProtection() }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { null }
    fun sessionToken(): String? = trust?.session
    suspend fun startReEnrollment() = mutex.withLock {
        keys.clear()
        pinUnlocked = !pinRequired
        trust = null
        api?.close()
        api = null
        transition(TrustState.Enrollment)
    }
}
