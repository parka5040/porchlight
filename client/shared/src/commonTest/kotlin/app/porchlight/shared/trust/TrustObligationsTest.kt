package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.serialization.json.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import kotlin.test.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Instant

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class TrustObligationsTest {
    private val start = Instant.parse("2026-10-05T12:00:00Z")
    private val uri = "porchlight://enroll?host=lan&token=test-token&ca=sha256:${"ab".repeat(32)}"
    private fun json(s: String) = WireJson.parseToJsonElement(s).jsonObject
    private fun trusted(expiry: Instant) = DeviceTrust("device", "leaf", "root", "session",
        "2028-12-01T00:00:00Z", listOf("https://lan:8443"), expiry.toString())
    // Copy durable fields into a new store instance to simulate process death (not the CSR cache).
    private class Keys(var value: DeviceTrust? = null) : DeviceKeyStore {
        var csrCalls = 0
        var failAttemptSave = false
        var failTrustSaves = 0
        var walletFailsTrustSave = false
        var failAttemptClear = false
        var durableAttempt: String? = null
        override suspend fun loadEnrollmentAttempt(targetBinding: String?) = durableAttempt?.let { WireJson.decodeFromString<PendingEnrollments>(it).find(targetBinding) }
        override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) {
            if (failAttemptSave) error("storage unavailable")
            durableAttempt = WireJson.encodeToString((durableAttempt?.let { WireJson.decodeFromString<PendingEnrollments>(it) } ?: PendingEnrollments()).with(attempt))
        }
        override suspend fun clearEnrollmentAttempt(targetBinding: String?) {
            if (failAttemptClear) error("storage unavailable")
            durableAttempt = durableAttempt?.let { WireJson.encodeToString(WireJson.decodeFromString<PendingEnrollments>(it).without(targetBinding)) }
        }
        override suspend fun load() = value
        override suspend fun csr() = "randomized-csr-${++csrCalls}"
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) {
            if (walletFailsTrustSave) throw WalletUnavailable()
            if (failTrustSaves-- > 0) error("storage unavailable")
            value = trust; durableAttempt = null
        }
        override suspend fun clear() { value = null }
        override fun certificateExpiry(pem: String) = "2027-10-05T12:00:00Z"
        fun reload() = Keys(value).also { it.csrCalls = csrCalls; it.durableAttempt = durableAttempt }
    }
    private class LostReply : Exception("Reply lost after write")
    private inner class Server : TrustTransportFactory, ApiTransport {
        private var committed: Pair<String, JsonObject>? = null
        private val rejected = mutableMapOf<String, Pair<JsonObject, ApiProblem>>()
        var firstFailure: Exception = LostReply()
        var inProgress = false
        var offline: Exception? = null // provably unsent, even after an earlier commit
        var commitBeforeFailure = true
        var renewalTooEarly = false
        var meGate: CompletableDeferred<Unit>? = null
        var meCalls = 0
        var endpoints = listOf("https://lan:8443")
        var enrollmentCalls = 0
        var renewalCalls = 0
        var failedRenewals = 0
        var authenticatedCalls = 0
        var failAuthenticatedOnCall = 0
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust): ApiTransport {
            if (++authenticatedCalls == failAuthenticatedOnCall) throw WalletUnavailable()
            return this
        }
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path) {
            "/v1/enrollment/complete" -> {
                enrollmentCalls++
                offline?.let { offline = null; throw it }
                // HeaderPolicy mints a new key when caller omits it, as production does.
                val key = idempotencyKey ?: "fresh-header-key-$enrollmentCalls"
                committed?.let {
                    if (it.first != key) throw ApiProblem("enrollment-token-used", 410, "Used")
                    if (it.second != body) throw ApiProblem("idempotency-key-reused", 422, "Mismatch")
                    if (inProgress) { inProgress = false; throw ApiProblem("idempotency-in-progress", 409, "Pending", retryAfterMillis = 1000) }
                } ?: run {
                    rejected[key]?.let {
                        if (it.first != body) throw ApiProblem("idempotency-key-reused", 422, "Mismatch")
                        throw it.second
                    }
                    val failure = firstFailure
                    firstFailure = LostReply()
                    if (failure is ConnectFailure || failure is RequestNotSent || (failure is DeviceCertificateRevoked && failure.requestNotSent)) throw failure // provably unsent
                    if (failure is ApiProblem) {
                        rejected[key] = checkNotNull(body) to failure
                        throw failure // sealed, but token not consumed
                    }
                    if (!commitBeforeFailure) { commitBeforeFailure = true; throw failure }
                    committed = key to checkNotNull(body)
                    throw failure // commit succeeded; reply disappeared
                }
                json("""{"device_id":"device","certificate":"leaf","ca_certificate":"root","session":{"token":"session","expires_at":"2028-12-01T00:00:00Z"},"endpoints":["https://lan:8443"]}""")
            }
            "/v1/me" -> { meCalls++; meGate?.await(); json("""{"member_id":"member","session_expires_at":"2028-12-01T00:00:00Z"}""") }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(endpoints.map(::JsonPrimitive))) }
            "/v1/auth/logout" -> buildJsonObject {}
            "/v1/devices/self/certificate" -> {
                renewalCalls++
                if (renewalTooEarly) throw ApiProblem("renewal-too-early", 409, "Not due")
                if (failedRenewals-- > 0) throw ConnectFailure()
                json("""{"certificate":"renewed-leaf"}""")
            }
            else -> error("Unexpected test route")
        }
    }
    @Test fun enrollmentLostSuccessReplaysInSameProcess() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, keys.csrCalls)
            assertEquals(2, server.enrollmentCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun enrollmentLostSuccessReplaysAfterProcessDeath() = runTest {
        val server = Server(); val keys = Keys()
        val dispatcher = StandardTestDispatcher(testScheduler)
        val first = TrustController(keys, server, "desktop", { start }, false, dispatcher)
        assertFailsWith<LostReply> { first.enroll(uri, "password") }
        first.close(); runCurrent()
        val reloaded = keys.reload()
        val second = TrustController(reloaded, server, "desktop", { start }, false, dispatcher)
        try {
            second.restore()
            second.enroll(uri, "password")
            assertEquals(TrustState.Ready, second.state.value)
            assertEquals(1, reloaded.csrCalls, "Process death must preserve exact randomized CSR")
        } finally { second.close(); runCurrent() }
    }
    @Test fun enrollmentMetadataCommitPrecedesAnySendAndContainsNoCredentials() = runTest {
        val server = Server(); val keys = Keys().also { it.failAttemptSave = true }
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<IllegalStateException> { controller.enroll(uri, "password") }
            assertEquals(0, server.enrollmentCalls)
            keys.failAttemptSave = false
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            val metadata = checkNotNull(keys.durableAttempt)
            assertFalse(metadata.contains("test-token"))
            assertFalse(metadata.contains("password"))
            assertFalse(metadata.contains("porchlight://"))
            val attempt = checkNotNull(keys.loadEnrollmentAttempt())
            assertEquals(keys.enrollmentKeyAlias, attempt.keyAlias)
            assertTrue(Regex("[A-Za-z0-9_-]{22}").matches(attempt.idempotencyKey))
            assertFalse(attempt.toString().contains(attempt.csr))
        } finally { controller.close(); runCurrent() }
    }
    @Test fun unsavedSuccessWithLockedWalletKeepsAttemptAndRestoresBeforeReplay() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            val original = keys.durableAttempt
            keys.walletFailsTrustSave = true
            assertFailsWith<WalletUnavailable> { controller.enroll(uri, "password") }
            assertEquals(TrustState.WalletLocked, controller.state.value)
            assertEquals(original, keys.durableAttempt); assertNull(keys.value)
            keys.walletFailsTrustSave = false
            controller.restore()
            assertEquals(TrustState.Enrollment, controller.state.value)
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertNull(keys.durableAttempt); assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun definitiveSuccessKeepsAttemptWhenLocalTrustSaveFailsAndRetryReplaysAndSaves() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore() // the UI enters Enrollment before submitting
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            keys.failTrustSaves = 1
            assertFailsWith<IllegalStateException> { controller.enroll(uri, "password") }
            assertNotNull(keys.loadEnrollmentAttempt(), "R78a: success not saved locally keeps the attempt")
            assertEquals(TrustState.Enrollment, controller.state.value, "ordinary unsaved success stays in Enrollment")
            assertNull(keys.value)
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertNotNull(keys.value)
            assertNull(keys.loadEnrollmentAttempt())
            assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun startOverClearsOnlyLocalPendingState() = runTest {
        val server = Server(); val keys = Keys(trusted(start + 90.days))
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            controller.startOverEnrollment()
            assertNull(keys.loadEnrollmentAttempt())
            assertNotNull(keys.value)
            assertEquals(1, server.enrollmentCalls, "Start over does not call the server")
        } finally { controller.close(); runCurrent() }
    }
    @Test fun inProgressKeepsAttemptUntilCommittedOutcomeCanReplay() = runTest {
        val server = Server().also { it.inProgress = true }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            val original = keys.durableAttempt
            assertEquals("idempotency-in-progress", assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }.type)
            assertEquals(original, keys.durableAttempt)
            advanceTimeBy(1000)
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun sealedAccountLockClearsAttemptAndRetryAfterLockoutWorks() = runTest {
        val server = Server().also { it.firstFailure = ApiProblem("account-locked", 429, "Locked") }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertEquals("account-locked", assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }.type)
            assertNull(keys.loadEnrollmentAttempt())
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun neverSentRequestAllowsCorrectedPassword() = runTest {
        val server = Server().also { it.firstFailure = ConnectFailure() }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<ConnectFailure> { controller.enroll(uri, "typo") }
            assertNull(keys.loadEnrollmentAttempt())
            assertFailsWith<LostReply> { controller.enroll(uri, "correct") }
            controller.enroll(uri, "correct")
            assertEquals(TrustState.Ready, controller.state.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun changedPasswordResendsStoredKeyAndDefinitiveAnswerDoesNotDeadEnd() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            assertEquals("idempotency-key-reused", assertFailsWith<ApiProblem> { controller.enroll(uri, "different") }.type)
            assertEquals(2, server.enrollmentCalls, "Let the server decide, not a local password verifier")
            assertNotNull(keys.loadEnrollmentAttempt(), "422 only proves the first request was processed")
            val again = assertFailsWith<ApiProblem> { controller.enroll(uri, "different") }
            assertEquals("idempotency-key-reused", again.type)
            assertTrue(again.title.contains("original password") && again.title.contains("Start over"))
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun offlineRetryAfterLostReplyKeepsStoredAttemptAndOnlineRetryReplays() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            val original = keys.durableAttempt
            server.offline = ConnectFailure()
            assertFailsWith<ConnectFailure> { controller.enroll(uri, "password") }
            assertEquals(original, keys.durableAttempt, "NB-1: a loaded attempt may have committed earlier")
            controller.enroll(uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun storedAttemptSurvivesPreProcessingRejections() = runTest {
        for ((type, status) in listOf("rate-limited" to 429, "request-timeout" to 408, "upgrade-required" to 426)) {
            val server = Server(); val keys = Keys()
            val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
            try {
                assertFailsWith<LostReply> { controller.enroll(uri, "password") }
                val original = keys.durableAttempt
                server.offline = ApiProblem(type, status, "rejected")
                assertEquals(type, assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }.type)
                assertEquals(original, keys.durableAttempt, "N-1: $type says nothing about the earlier committed send")
                advanceTimeBy(61_000)
                controller.enroll(uri, "password")
                assertEquals(TrustState.Ready, controller.state.value)
                assertEquals(1, keys.csrCalls)
            } finally { controller.close(); runCurrent() }
        }
    }
    @Test fun storedAttemptIsClearedOnDefinitiveRejection() = runTest {
        val server = Server(); val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "password") }
            server.offline = ApiProblem("enrollment-token-invalid", 410, "gone")
            assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }
            assertNull(keys.loadEnrollmentAttempt())
        } finally { controller.close(); runCurrent() }
    }
    @Test fun attemptClearFailureNeverMasksServerProblem() = runTest {
        val server = Server().also { it.firstFailure = ApiProblem("account-locked", 429, "Locked") }
        val keys = Keys().also { it.failAttemptClear = true }
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertEquals("account-locked", assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }.type)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun changedPasswordOnUncommittedUnknownOutcomeStillUsesStoredKey() = runTest {
        val server = Server().also { it.commitBeforeFailure = false }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<LostReply> { controller.enroll(uri, "typo") }
            val original = keys.loadEnrollmentAttempt()?.idempotencyKey
            assertFailsWith<LostReply> { controller.enroll(uri, "correct") }
            assertEquals(original, keys.loadEnrollmentAttempt()?.idempotencyKey)
            controller.enroll(uri, "correct")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, keys.csrCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun handshakeFailureIsProvablyUnsentAndDropsAttempt() = runTest {
        val server = Server().also { it.firstFailure = RequestNotSent() }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<RequestNotSent> { controller.enroll(uri, "typo") }
            assertNull(keys.loadEnrollmentAttempt())
            assertFailsWith<LostReply> { controller.enroll(uri, "correct") }
            controller.enroll(uri, "correct")
            assertEquals(TrustState.Ready, controller.state.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun explicitRevocationDuringHandshakeIsAlsoUnsent() = runTest {
        val server = Server().also { it.firstFailure = DeviceCertificateRevoked(requestNotSent = true) }; val keys = Keys()
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            assertFailsWith<DeviceCertificateRevoked> { controller.enroll(uri, "password") }
            assertNull(keys.loadEnrollmentAttempt())
        } finally { controller.close(); runCurrent() }
    }
    @Test fun definitiveFourHundredsClearWhileFiveHundredsRetainIdentity() = runTest {
        for ((status, type) in listOf(408 to "http-error", 429 to "rate-limited", 409 to "conflict", 410 to "enrollment-token-expired", 503 to "unavailable")) {
            val server = Server().also { it.firstFailure = ApiProblem(type, status, "Failed") }; val keys = Keys()
            val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
            try {
                assertFailsWith<ApiProblem> { controller.enroll(uri, "password") }
                assertEquals(status >= 500, keys.loadEnrollmentAttempt() != null)
            } finally { controller.close(); runCurrent() }
        }
    }
    @Test fun nearExpiryRenewalWarningAppearsWithoutNaggingEarlierTransientFailures() = runTest {
        for (daysLeft in listOf(6, 29)) {
            val server = Server().also { it.failedRenewals = 1 }
            val controller = TrustController(Keys(trusted(start + daysLeft.days)), server, "desktop",
                { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
            try {
                controller.restore(); controller.refreshWhenReady(); runCurrent()
                assertEquals(daysLeft == 6, controller.renewalWarning.value != null)
                assertNull(controller.actionError.value)
                advanceTimeBy(60_000); runCurrent()
                assertNull(controller.renewalWarning.value, "Successful renewal clears warning")
            } finally { controller.close(); runCurrent() }
        }
    }
    @Test fun stableRefreshPreservesTransportAndEndpointChangeRebuildsIt() = runTest {
        val server = Server(); val keys = Keys(trusted(start + 90.days))
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore()
            repeat(3) { controller.refreshEndpointsAndRenew() }
            assertEquals(1, server.authenticatedCalls)
            server.endpoints = listOf("https://other:8443")
            controller.refreshEndpointsAndRenew()
            assertEquals(2, server.authenticatedCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun hungBackgroundRefreshDoesNotHoldBusyOrMutexAndLateReplyCannotUndoLogout() = runTest {
        val gate = CompletableDeferred<Unit>()
        val server = Server().also { it.meGate = gate }; val keys = Keys(trusted(start + 90.days))
        val controller = TrustController(keys, server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.meCalls)
            assertFalse(controller.busy.value)
            val logout = assertNotNull(controller.runAction { controller.logout() })
            runCurrent()
            assertTrue(logout.isCompleted, "Logout must not wait for a hung refresh GET")
            assertEquals(TrustState.Login, controller.state.value)
            gate.complete(Unit); runCurrent()
            assertEquals(TrustState.Login, controller.state.value)
            assertNull(keys.value?.session, "Stale refresh must not resurrect a session")
        } finally { controller.close(); runCurrent() }
    }
    @Test fun foregroundAndUnlockRecheckOverdueWallClockDeadlineWithoutResettingBackoff() = runTest {
        var jump = 0L
        val server = Server().also { it.failedRenewals = 2 }
        val controller = TrustController(Keys(trusted(start + 30.days + 1_000.milliseconds)), server, "android",
            { start + (testScheduler.currentTime + jump).milliseconds }, true, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.unlockPin(); controller.refreshWhenReady(); runCurrent()
            assertEquals(0, server.renewalCalls)
            jump = 2_000 // suspend crossed wall-clock deadline without advancing monotonic coroutine delay
            controller.onForeground(); runCurrent()
            assertEquals(1, server.renewalCalls)
            advanceTimeBy(60_000); runCurrent()
            assertEquals(2, server.renewalCalls)
            controller.lockPin(); controller.unlockPin(); controller.onForeground(); runCurrent()
            advanceTimeBy(119_999); runCurrent()
            assertEquals(2, server.renewalCalls)
            advanceTimeBy(1); runCurrent()
            assertEquals(3, server.renewalCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun renewalTooEarlyUsesDailyCadenceInsteadOfFailureBackoff() = runTest {
        val server = Server().also { it.renewalTooEarly = true }
        val controller = TrustController(Keys(trusted(start + 20.days)), server, "desktop",
            { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.renewalCalls)
            assertNull(controller.renewalWarning.value)
            advanceTimeBy(1.days.inWholeMilliseconds - 1); runCurrent()
            assertEquals(1, server.renewalCalls)
            advanceTimeBy(1); runCurrent()
            assertEquals(2, server.renewalCalls)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun renewalFailureRetriesWithBackoffWithoutAnotherLogin() = runTest {
        val server = Server().also { it.failedRenewals = 1 }
        val keys = Keys(trusted(start + 29.days))
        val controller = TrustController(keys, server, "desktop", { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.renewalCalls)
            advanceTimeBy(59_999); runCurrent()
            assertEquals(1, server.renewalCalls, "No retry spin")
            advanceTimeBy(1); runCurrent()
            assertEquals(2, server.renewalCalls, "Retry at one minute without another refresh call")
            assertEquals("renewed-leaf", keys.value?.certificate)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun renewalRetriesTransportRebuildAfterCertificateWasSaved() = runTest {
        val server = Server().also { it.failAuthenticatedOnCall = 2 }
        val keys = Keys(trusted(start + 29.days))
        val controller = TrustController(keys, server, "desktop", { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals("renewed-leaf", keys.value?.certificate)
            assertEquals(2, server.authenticatedCalls)
            advanceTimeBy(60_000); runCurrent()
            assertEquals(3, server.authenticatedCalls, "Reconnect once before GET; unchanged refresh preserves the live transport")
            assertEquals(1, server.renewalCalls, "Already saved certificate must not be reissued")
        } finally { controller.close(); runCurrent() }
    }
    @Test fun renewalBackoffDoublesAndPinChangesDoNotResetItAndCloseStopsIt() = runTest {
        val server = Server().also { it.failedRenewals = 10 }
        val controller = TrustController(Keys(trusted(start + 29.days)), server, "android",
            { start + testScheduler.currentTime.milliseconds }, true, StandardTestDispatcher(testScheduler))
        controller.restore(); controller.unlockPin(); controller.refreshWhenReady(); runCurrent()
        assertEquals(1, server.renewalCalls)
        advanceTimeBy(60_000); runCurrent()
        assertEquals(2, server.renewalCalls)
        controller.lockPin(); controller.unlockPin(); controller.refreshWhenReady()
        advanceTimeBy(119_999); runCurrent()
        assertEquals(2, server.renewalCalls)
        advanceTimeBy(1); runCurrent()
        assertEquals(3, server.renewalCalls)
        controller.close(); runCurrent()
        advanceTimeBy(1.days.inWholeMilliseconds); runCurrent()
        assertEquals(3, server.renewalCalls)
    }
    @Test fun longLivedSessionRenewsWhenThresholdIsCrossedAndRearms() = runTest {
        val server = Server(); val keys = Keys(trusted(start + 30.days + 1_000.milliseconds))
        val controller = TrustController(keys, server, "android", { start + testScheduler.currentTime.milliseconds }, true, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.unlockPin(); controller.refreshWhenReady(); runCurrent()
            assertEquals(0, server.renewalCalls)
            controller.lockPin() // R64: scheduled obligation survives UI-only relock
            advanceTimeBy(1_001); runCurrent()
            assertEquals(1, server.renewalCalls, "Threshold crossing in a live session")
            assertEquals(TrustState.PinRequired, controller.state.value)
            advanceTimeBy(60_000); runCurrent()
            assertEquals(1, server.renewalCalls, "New certificate re-arms the threshold")
        } finally { controller.close(); runCurrent() }
    }
}
