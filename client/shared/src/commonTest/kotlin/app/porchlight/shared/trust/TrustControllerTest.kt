package app.porchlight.shared.trust

import app.porchlight.shared.api.models.ItemResponse
import app.porchlight.shared.transport.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.data.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.runCurrent
import kotlinx.serialization.json.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import kotlin.test.*
import kotlin.time.Instant

class TrustControllerTest {
    private val now = Instant.parse("2026-10-05T12:00:00Z")
    private fun value(session: String? = "session", expiry: String = "2027-10-01T00:00:00Z") = DeviceTrust("self", "leaf", "root", session,
        if (session == null) null else "2026-12-01T00:00:00Z", listOf("https://lan:8443"), expiry)
    private class Keys(var value: DeviceTrust?) : DeviceKeyStore {
        var locked = false; var csrCalls = 0; var pin: String? = null
        var durableAttempt: String? = null
        override suspend fun loadEnrollmentAttempt(targetBinding: String?) = durableAttempt?.let { WireJson.decodeFromString<PendingEnrollments>(it).find(targetBinding) }
        override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) { durableAttempt = WireJson.encodeToString((durableAttempt?.let { WireJson.decodeFromString<PendingEnrollments>(it) } ?: PendingEnrollments()).with(attempt)) }
        override suspend fun clearEnrollmentAttempt(targetBinding: String?) {
            durableAttempt = durableAttempt?.let { WireJson.encodeToString(WireJson.decodeFromString<PendingEnrollments>(it).without(targetBinding)) }
        }
        override suspend fun load(): DeviceTrust? { if (locked) throw WalletUnavailable(); return value }
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust; durableAttempt = null; pin = expectedPin }
        override suspend fun clear() { value = null }
        override suspend fun csr(): String { if (locked) throw WalletUnavailable(); csrCalls++; return "CSR" }
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private class Factory : TrustTransportFactory, ApiTransport {
        var result = buildJsonObject {}
        var problem: Exception? = null
        var handler: (suspend (String) -> JsonObject)? = null
        var requestVersion: Long? = null
        var streamHandler: (suspend (suspend (StreamFrame) -> Unit) -> Unit)? = null
        override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) { streamHandler?.invoke(receive) ?: awaitCancellation() }
        var path = ""; var body: JsonObject? = null; var session: String? = null; var step: String? = null; var key: String? = null
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
            this.requestVersion = version; this.path = path; this.body = body; this.session = session; this.step = stepUp; this.key = idempotencyKey
            problem?.let { throw it }
            return handler?.invoke(path) ?: result
        }
    }
    private fun json(text: String) = WireJson.parseToJsonElement(text).jsonObject
    private fun snapshot(cursor: Long, vararg items: String) =
        WireJson.encodeToJsonElement(emptySnapshot(cursor).copy(items = items.map { itemRead(it) })).jsonObject
    /** A household server: [override] answers first (null = default), then household defaults. */
    private fun household(override: suspend (String) -> JsonObject? = { null }): suspend (String) -> JsonObject = { full ->
        val path = full.substringBefore('?')
        override(path) ?: when (path) {
            "/v1/board", "/v1/agenda", "/v1/notifications" -> json("""{"items":[],"next_cursor":null}""")
            "/v1/me" -> json("""{"member_id":"member","session_expires_at":"2027-04-01T00:00:00Z"}""")
            "/v1/endpoints" -> json("""{"endpoints":["https://lan:8443"]}""")
            "/v1/auth/login" -> json("""{"session":{"token":"replacement","expires_at":"2027-01-01T00:00:00Z"}}""")
            else -> buildJsonObject {}
        }
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun optedOutBackgroundBindingMakesNoSnapshotOrStreamAndForegroundIsBounded() = runTest {
        var snapshots = 0; var connects = 0
        val connectCount = kotlinx.coroutines.flow.MutableStateFlow(0)
        val connected = CompletableDeferred<Unit>()
        val factory = Factory().also {
            it.handler = household { path -> if (path == "/v1/snapshot") { snapshots++; snapshot(51, "one") } else null }
            it.streamHandler = { connects++; connectCount.value = connects; connected.complete(Unit); awaitCancellation() }
        }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val enabled = kotlinx.coroutines.flow.MutableStateFlow(false)
        val binding = data.bindTrust(controller, backgroundScope, streamEnabled = enabled, refreshWhenInactive = false, resumeFromCursor = true)
        try {
            controller.restore(); runCurrent()
            assertEquals(0, snapshots); assertEquals(0, connects)
            enabled.value = true; connected.await()
            assertEquals(1, snapshots); assertEquals(1, connects)
            enabled.value = false; runCurrent()
            withBackgroundSession(controller) { data.sync.refresh() } // one WorkManager request, no bind snapshot
            assertEquals(2, snapshots); assertEquals(1, connects)
            enabled.value = true; connectCount.first { it == 2 }
            assertEquals(2, snapshots) // resume from saved cursor, not another snapshot
            controller.lockPin(); runCurrent()
            assertEquals(2, snapshots)
        } finally { binding.cancel(); controller.close(); data.close() }
    }
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    @Test fun streamDispatchEvictsRefetchesNotificationsAndSnapshotsItemsOnceBeforeAck() = runTest {
        val paths = mutableListOf<String>()
        val connected = CompletableDeferred<Unit>()
        val frames = kotlinx.coroutines.channels.Channel<StreamFrame>(kotlinx.coroutines.channels.Channel.UNLIMITED)
        val factory = Factory().also {
            it.handler = household { path -> paths += path; when (path) {
                "/v1/snapshot" -> snapshot(51, "one", "two")
                "/v1/notifications" -> json("""{"items":[],"next_cursor":null}""")
                else -> null
            } }
            it.streamHandler = { receive -> connected.complete(Unit); for (frame in frames) receive(frame) }
        }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore(); connected.await(); paths.clear()
            frames.send(StreamFrame.Tickle(52, "item.access_changed", "item", "one", true)); cache.sync.first { it?.cursor == 52L }
            assertFalse(cache.view.value.items.any { it.id == "one" }); assertEquals(52, cache.sync.value?.cursor)
            assertFalse("/v1/snapshot" in paths)
            paths.clear()
            frames.send(StreamFrame.Tickle(53, "notification.dismissed", "notification", "n", false)); cache.sync.first { it?.cursor == 53L }
            assertEquals(listOf("/v1/notifications"), paths); assertEquals(53, cache.sync.value?.cursor)
            paths.clear()
            frames.send(StreamFrame.Tickle(54, "item.changed", "item", "two", false)); cache.sync.first { it?.cursor == 54L }
            assertEquals(1, paths.count { it == "/v1/snapshot" })
            assertEquals(1, paths.count { it == "/v1/board" }); assertEquals(1, paths.count { it == "/v1/agenda" })
            assertFalse(paths.any { it.startsWith("/v1/items/") }); assertEquals(54, cache.sync.value?.cursor)
            paths.clear()
            frames.send(StreamFrame.Tickle(55, "derived", "item", "two", false)); cache.sync.first { it?.cursor == 55L }
            assertEquals(1, paths.count { it == "/v1/snapshot" })
            paths.clear()
            frames.send(StreamFrame.Tickle(56, "settings.changed", "member", "member", false)); cache.sync.first { it?.cursor == 56L }
            assertEquals(1, paths.count { it == "/v1/snapshot" }) // canonical settings tickle includes prefs
        } finally { binding.cancel(); controller.close(); data.close() }
    }
    // N-1 (b) / R81: the terminal stream `resnapshot` frame is an authoritative replace. A response received while
    // that snapshot loads is NOT re-applied over it (an ordinary refresh would re-apply it).
    @Test fun streamResnapshotFrameInstallsAuthoritativelyWithoutReapplyingReceipts() = runTest {
        val connected = CompletableDeferred<Unit>()
        val frames = kotlinx.coroutines.channels.Channel<StreamFrame>(kotlinx.coroutines.channels.Channel.UNLIMITED)
        val cache = memoryCache { now }
        var snapshots = 0
        val factory = Factory().also {
            it.handler = household { path -> if (path != "/v1/snapshot") null else {
                snapshots++
                // From the second snapshot on, an own write response lands after the request, before the install.
                if (snapshots > 1) cache.putItem(itemResponse("one").copy(version = 5, title = "Own write"))
                snapshot(50L + snapshots * 10, "one")
            } }
            it.streamHandler = { receive -> connected.complete(Unit); for (frame in frames) receive(frame) }
        }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore(); connected.await()
            assertEquals(1, snapshots); assertEquals(60, cache.sync.value?.cursor)
            frames.send(StreamFrame.Resnapshot); cache.sync.first { it?.cursor == 70L }
            assertEquals(2, snapshots)
            assertEquals(1, cache.view.value.items.single().version)
            assertEquals("Title", cache.view.value.items.single().title)
            // Control: an ordinary (non-authoritative) refresh keeps the post-request receipt (c2b N-1).
            withBackgroundSession(controller) { data.sync.refresh() }
            assertEquals(80, cache.sync.value?.cursor)
            assertEquals(5, cache.view.value.items.single().version)
        } finally { binding.cancel(); controller.close(); data.close() }
    }
    // C-1: production order is bind (composition, TrustedPorchlightApp) and THEN restore (TrustGate's LaunchedEffect).
    @Test fun bindBeforeOfflineRestorePreservesPersistedCacheAndSessionExpiry() = runTest {
        val keys = Keys(value()); val factory = Factory().also { it.problem = ConnectFailure() }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        cache.initialize(value().cacheOwner()) // Persisted fixtures must belong to the restored identity.
        cache.install(emptySnapshot(41).copy(items = listOf(itemRead("persisted"))))
        val prior = cache.view.value
        // Actual application order: data binds in composition, TrustGate restores afterwards.
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore()
            data.sync.status.first { it is SyncStatus.Failed }
            assertEquals(prior, cache.view.value)
            val epoch = controller.sessionEpoch.value
            controller.onProblem(ApiProblem("session-invalid", 401, ""))
            assertEquals(epoch, controller.sessionEpoch.value)
            cache.initialize(); assertEquals(prior, cache.view.value)
            controller.close()
            assertEquals(epoch, controller.sessionEpoch.value)
        } finally { binding.cancelAndJoin(); controller.close(); cache.close() }
    }
    @Test fun coldStartAfterExpiryAndInSessionExpiryReloginKeepSameIdentityCache() = runTest {
        // Session expiry persists session=null, so the next cold start restores into Login, not SessionExpired.
        val keys = Keys(value(session = null)); val factory = Factory()
        val snapshots = kotlinx.coroutines.flow.MutableStateFlow(0)
        factory.handler = household { path -> if (path == "/v1/snapshot") { snapshots.value++; throw ConnectFailure() } else null }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        cache.initialize(value().cacheOwner()) // Persisted fixtures must belong to the restored identity.
        cache.install(emptySnapshot(41).copy(items = listOf(itemRead("persisted"))))
        val prior = cache.view.value
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore()
            assertEquals(TrustState.Login, controller.state.value)
            controller.login("password")
            snapshots.first { it == 1 }; data.sync.status.first { it is SyncStatus.Failed }
            assertEquals(prior, cache.view.value)
            controller.onProblem(ApiProblem("session-invalid", 401, ""))
            assertEquals(TrustState.SessionExpired, controller.state.value)
            controller.login("password")
            // StateFlow may conflate Ready -> SessionExpired -> Ready for the binding: refresh explicitly (C3 pull).
            assertFailsWith<SyncUnavailable> { data.sync.refresh() }
            assertEquals(prior, cache.view.value)
            cache.initialize(); assertEquals(prior, cache.view.value)
        } finally { binding.cancelAndJoin(); controller.close(); cache.close() }
    }
    @Test fun epochAdvancesOnlyWhenTheIdentityEnds() = runTest {
        val keys = Keys(value()); val factory = Factory()
        factory.handler = household()
        val controller = TrustController(keys, factory, "android", { now }, true)
        try {
            controller.restore(); controller.unlockPin()
            val epoch = controller.sessionEpoch.value
            controller.restore(); controller.unlockPin()           // cold-start style re-restore of the same identity
            controller.lockPin(); controller.unlockPin()           // PIN relock
            val identity = controller.notificationIdentity()
            controller.refreshEndpointsAndRenew()                  // identity learnt from /v1/me
            assertEquals(identity, controller.notificationIdentity())
            controller.onProblem(ApiProblem("session-invalid", 401, ""))
            controller.login("password")                           // same device/member, new session token
            assertEquals(identity, controller.notificationIdentity())
            controller.onProblem(ApiProblem("upgrade-required", 426, ""))
            controller.restore(); controller.unlockPin()
            assertEquals(epoch, controller.sessionEpoch.value)
            controller.logout()
            assertEquals(epoch + 1, controller.sessionEpoch.value)
            controller.login("password")
            assertEquals(epoch + 1, controller.sessionEpoch.value)
            controller.onProblem(ApiProblem("device-revoked", 401, ""))
            assertEquals(epoch + 2, controller.sessionEpoch.value)
            controller.startReEnrollment()
            assertEquals(epoch + 3, controller.sessionEpoch.value)
        } finally { controller.close() }
    }
    @Test fun sessionEpochChangesEvenWhenReadyIsConflated() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore()
        val epoch = controller.sessionEpoch.value
        controller.logout()
        factory.result = WireJson.parseToJsonElement("""{"session":{"token":"replacement","expires_at":"2027-01-01T00:00:00Z"}}""").jsonObject
        controller.login("password")
        assertEquals(TrustState.Ready, controller.state.value)
        assertTrue(controller.sessionEpoch.value > epoch)
        controller.close()
    }
    @Test fun readyBindingSyncsOnlyAfterUnlockAndLogoutAndReenrollClear() = runTest {
        val keys = Keys(value()); val factory = Factory()
        var offline = true
        factory.handler = household { path -> if (path == "/v1/snapshot") { if (offline) throw ConnectFailure(); snapshot(42, "fresh") } else null }
        val controller = TrustController(keys, factory, "android", { now }, true)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        cache.initialize(value().cacheOwner()) // Persisted fixtures must belong to the restored identity.
        cache.install(emptySnapshot(41).copy(items = listOf(itemRead("persisted"))))
        val binding = data.bindTrust(controller, backgroundScope) // production order: bind, then restore
        try {
            controller.restore()
            assertEquals(TrustState.PinRequired, controller.state.value)
            controller.unlockPin()
            // The binding saw PinRequired, then Ready, and its offline Ready sync failed. Neither restore nor the PIN
            // gate changes the identity epoch, so nothing cleared the persisted cache in between.
            data.sync.status.first { it is SyncStatus.Failed }
            assertEquals(41, cache.sync.value?.cursor) // restore + PIN gate hide, never discard, the persisted cache
            offline = false
            data.sync.refresh() // explicit: the binding does not retry its failed offline Ready sync
            data.sync.status.first { it == SyncStatus.Ready }
            assertEquals(42, cache.sync.value?.cursor)
            controller.logout()
            cache.view.first { it.sync == null && it.items.isEmpty() }
            assertEquals(TrustState.Login, controller.state.value)
            assertFailsWith<CacheOwnerUnavailable> { cache.install(emptySnapshot(43)) }
            controller.login("password") // Readmit the owner before seeding data for the revocation check.
            cache.install(emptySnapshot(43))
            controller.onProblem(ApiProblem("device-revoked", 401, ""))
            cache.view.first { it.sync == null }
            assertEquals(TrustState.ReEnroll, controller.state.value)
        } finally { binding.cancelAndJoin(); controller.close(); cache.close() }
    }
    @Test fun productionLogoutClearsAndFencesLateSnapshotFromPreviousSession() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val staleStarted = CompletableDeferred<Unit>(); val releaseStale = CompletableDeferred<Unit>()
        var snapshots = 0
        factory.handler = household { path ->
            if (path != "/v1/snapshot") null
            else when (++snapshots) {
                1 -> snapshot(42, "first")
                // A C3 pull-to-refresh in its own scope, answered only after logout and relogin.
                2 -> { staleStarted.complete(Unit); withContext(NonCancellable) { releaseStale.await() }; snapshot(99, "stale") }
                else -> snapshot(43, "fresh")
            }
        }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val cursors = mutableListOf<Long?>()
        backgroundScope.launch { cache.sync.collect { cursors += it?.cursor } }
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore()
            cache.sync.first { it?.cursor == 42L }
            val stale = async(Dispatchers.Default) { data.sync.refresh() }
            staleStarted.await()
            controller.logout()
            cache.view.first { it.sync == null && it.items.isEmpty() }
            controller.login("password")
            releaseStale.complete(Unit)
            assertFailsWith<CancellationException> { stale.await() }
            cache.sync.first { it?.cursor == 43L }
            assertEquals(listOf("fresh"), cache.view.value.items.map { it.id })
            assertFalse(99L in cursors)
        } finally { binding.cancelAndJoin(); controller.close(); cache.close() }
    }
    // O-1: logout's clear is durable before logout returns; a process death right after it leaves no data on disk.
    @Test fun logoutReturnsOnlyAfterTheCacheIsClearedOnDisk() = runTest {
        val keys = Keys(value()); val factory = Factory()
        factory.handler = household { path -> if (path == "/v1/snapshot") snapshot(42, "first") else null }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore()
            cache.sync.first { it?.cursor == 42L }
            controller.logout()
            binding.cancel() // the process dies as soon as logout returns: the binding never runs again
            cache.initialize() // what the next process would load from disk
            assertNull(cache.sync.value)
            assertTrue(cache.view.value.items.isEmpty())
        } finally { binding.cancelAndJoin(); controller.close(); cache.close() }
    }
    @Test fun lateProblemFromExpiredSessionCannotExpireSameIdentityRelogin() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        factory.handler = household { path ->
            if (path == "/v1/snapshot") { started.complete(Unit); release.await(); throw ApiProblem("session-invalid", 401, "") } else null
        }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        try {
            controller.restore()
            val call = GeneratedCall.trusted(controller)
            val old = async { call.request("GET", "/v1/snapshot", emptyMap(), emptyMap(), null) }
            started.await()
            controller.onProblem(ApiProblem("session-invalid", 401, "")) // another request saw the expiry first
            controller.login("password")
            assertEquals(TrustState.Ready, controller.state.value)
            release.complete(Unit)
            assertFailsWith<CancellationException> { old.await() }
            assertEquals(TrustState.Ready, controller.state.value)
        } finally { controller.close() }
    }
    @Test fun lockedColdStartRefreshesButUiAndSensitiveRequestsStayGated() = runTest {
        val factory = Factory().also { it.handler = household { path -> if (path == "/v1/snapshot") snapshot(51, "background") else null } }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore()
            cache.sync.first { it?.cursor == 51L }
            assertEquals(TrustState.PinRequired, controller.state.value)
            assertEquals("background", cache.view.value.items.single().id)
            assertFailsWith<HouseholdLocked> { data.sync.refresh() }
            withBackgroundSession(controller) {
                data.sync.refresh() // WorkManager uses the same scoped refresh
                assertFailsWith<HouseholdLocked> { GeneratedCall.trusted(controller).request("POST", "/v1/auth/password", emptyMap(), emptyMap(), buildJsonObject {}) }
                assertFailsWith<HouseholdLocked> { controller.reauth("secret", "revoke-device") }
            }
        } finally { binding.cancel(); controller.close(); cache.close() }
    }
    @Test fun pinRelockDoesNotCancelOrReconnectLiveStream() = runTest {
        val entered = CompletableDeferred<Unit>(); val tick = CompletableDeferred<Unit>(); val ack = CompletableDeferred<Unit>()
        var connects = 0
        val factory = Factory().also {
            it.handler = household { path -> if (path == "/v1/snapshot") snapshot(51, "live") else null }
            it.streamHandler = { receive ->
                connects++; entered.complete(Unit); tick.await()
                receive(StreamFrame.Tickle(52, "notification.dismissed", "notification", "n", false))
                ack.complete(Unit); awaitCancellation()
            }
        }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore(); controller.unlockPin(); entered.await()
            controller.lockPin(); tick.complete(Unit); ack.await()
            assertEquals(1, connects); assertEquals(52L, cache.sync.value?.cursor)
            assertEquals(TrustState.PinRequired, controller.state.value)
        } finally { binding.cancel(); controller.close(); cache.close() }
    }
    @Test fun rapidSameIdentityReloginReconnectsEvenWhenUiStateIsConflated() = runTest {
        val first = CompletableDeferred<Unit>(); val second = CompletableDeferred<Unit>(); var connections = 0
        val factory = Factory().also {
            it.handler = household { path -> if (path == "/v1/snapshot") snapshot(51, "same") else null }
            it.streamHandler = { if (++connections == 1) first.complete(Unit) else second.complete(Unit); awaitCancellation() }
        }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val binding = data.bindTrust(controller, backgroundScope)
        try {
            controller.restore(); first.await()
            val epoch = controller.sessionEpoch.value
            controller.onProblem(ApiProblem("session-invalid", 401, "")); controller.login("password")
            second.await()
            assertEquals(epoch, controller.sessionEpoch.value)
            assertEquals(TrustState.PinRequired, controller.state.value)
            assertEquals(2, connections)
        } finally { binding.cancel(); controller.close(); cache.close() }
    }
    @Test fun capturedBackgroundLeaseCannotSendWithNewIdentityCredentials() = runTest {
        val factory = Factory().also { it.handler = household() }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        try {
            controller.restore()
            withBackgroundSession(controller) {
                controller.logout(); controller.login("password")
                val last = factory.path
                assertFailsWith<CancellationException> { GeneratedCall.trusted(controller).request("POST", "/v1/notifications/n/read", emptyMap(), mapOf("Idempotency-Key" to "old-owner"), buildJsonObject {}) }
                assertEquals(last, factory.path) // no request used the replacement session
            }
        } finally { controller.close() }
    }
    @Test fun backgroundCompleteAndDismissKeepOriginalInputsAcrossRelock() = runTest {
        val factory = Factory(); val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val seen = mutableListOf<Triple<String, JsonObject?, String?>>()
        var completeVersion: Long? = null
        factory.handler = { path ->
            seen += Triple(path, factory.body, factory.key)
            if (path.endsWith("/complete")) { completeVersion = factory.requestVersion; started.complete(Unit); release.await() }
            buildJsonObject {}
        }
        try {
            controller.restore(); controller.unlockPin()
            val call = GeneratedCall.trusted(controller); val body = buildJsonObject {}
            val complete = async { withBackgroundSession(controller) { call.request("POST", "/v1/occurrences/o/complete", emptyMap(), mapOf("Idempotency-Key" to "original", "If-Match" to "7"), body) } }
            started.await(); controller.lockPin(); release.complete(Unit); complete.await()
            withBackgroundSession(controller) {
                repeat(2) { call.request("POST", "/v1/notifications/n/read", emptyMap(), mapOf("Idempotency-Key" to "dismiss-original"), body) }
            }
            assertEquals(7L, completeVersion)
            assertEquals(listOf("original", "dismiss-original", "dismiss-original"), seen.map { it.third })
            assertTrue(seen.all { it.second == body })
        } finally { controller.close() }
    }
    @Test fun logoutFencesLateLockedBackgroundResponseAndCacheCommit() = runTest {
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val factory = Factory().also { it.handler = household { path -> if (path == "/v1/snapshot") { started.complete(Unit); release.await(); snapshot(99, "late") } else null } }
        val controller = TrustController(Keys(value()), factory, "android", { now }, true)
        val cache = memoryCache { now }; val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        cache.sessionEpoch = { controller.sessionEpoch.value }; cache.sessionCommitMutex = controller.sessionCommitMutex
        controller.identityEnded = { cache.clear() }
        try {
            controller.restore()
            val request = async { runCatching { withBackgroundSession(controller) { data.sync.refresh() } } }
            started.await(); controller.logout(); release.complete(Unit)
            assertIs<CancellationException>(request.await().exceptionOrNull())
            assertNull(cache.sync.value); assertTrue(cache.view.value.items.isEmpty())
        } finally { controller.close(); cache.close() }
    }
    @Test fun pinRelockMidRequestDropsReplyAsLockedNotConnectionFailure() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        factory.handler = household { path -> if (path == "/v1/snapshot") { started.complete(Unit); release.await(); snapshot(50, "late") } else null }
        val controller = TrustController(keys, factory, "android", { now }, true)
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        try {
            controller.restore(); controller.unlockPin()
            val refresh = async { runCatching { data.sync.refresh() } } // a failed child would cancel the test scope
            started.await()
            controller.lockPin()
            release.complete(Unit)
            assertIs<HouseholdLocked>(refresh.await().exceptionOrNull())
            assertEquals(SyncStatus.Idle, data.sync.status.value)
            assertNull(cache.sync.value)
        } finally { controller.close(); cache.close() }
    }
    // N-2: a reply withheld by a relock may follow a committed write; the exception names the key a retry must reuse.
    @Test fun writeReplyWithheldByRelockIsRetriedWithTheSameKey() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val commits = mutableMapOf<String, JsonObject>() // server idempotency: one commit per key, replays get its reply
        factory.handler = household { path ->
            if (path != "/v1/items/one") null
            else {
                val key = checkNotNull(factory.key)
                val first = key !in commits
                val reply = commits.getOrPut(key) { WireJson.encodeToJsonElement(itemResponse("one").copy(title = "Renamed", version = 2)).jsonObject }
                if (first) { started.complete(Unit); release.await() }
                reply
            }
        }
        val controller = TrustController(keys, factory, "android", { now }, true)
        val cache = memoryCache { now }
        var minted = 0 // every default key is fresh, so a retry with a new key would commit a second time
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now }, WriteCoordinator(newKey = { "generated-${++minted}" }))
        data.items.afterWrite = {}
        try {
            controller.restore(); controller.unlockPin()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            // The caller relies on the default key; a failed child would cancel the test scope, hence runCatching.
            val edit = async { runCatching { data.items.edit("one", 1, ItemEdit().text("title", "Renamed")) } }
            started.await(); controller.lockPin(); release.complete(Unit)
            val withheld = assertIs<HouseholdLockedAfterReply>(edit.await().exceptionOrNull())
            assertIs<HouseholdLocked>(withheld) // sync and callers that only test for the gate still match
            assertEquals("generated-1", withheld.idempotencyKey)
            assertEquals("Title", cache.view.value.items.single().title) // the withheld reply was not applied
            controller.unlockPin()
            val retried = data.items.edit("one", 1, ItemEdit().text("title", "Renamed"), key = checkNotNull(withheld.idempotencyKey))
            assertIs<WriteResult.Success<ItemResponse>>(retried)
            assertEquals(1, commits.size) // committed once, the retry replayed it
            assertEquals("Renamed", cache.view.value.items.single().title)
        } finally { controller.close(); cache.close() }
    }
    @Test fun oldGeneratedReplyCannotAffectNewReadySession() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore()
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        factory.handler = { path ->
            if (path == "/v1/snapshot") { started.complete(Unit); release.await(); buildJsonObject {} }
            else if (path == "/v1/auth/login") WireJson.parseToJsonElement("""{"session":{"token":"new","expires_at":"2027-01-01T00:00:00Z"}}""").jsonObject
            else buildJsonObject {}
        }
        try {
            val call = GeneratedCall.trusted(controller)
            val old = async { call.request("GET", "/v1/snapshot", emptyMap(), emptyMap(), null) }
            started.await(); controller.logout(); controller.login("password"); release.complete(Unit)
            assertEquals(TrustState.Ready, controller.state.value)
            assertFailsWith<CancellationException> { old.await() }
        } finally { controller.close() }
    }
    @Test fun pinGatePrecedesRestoredContentAndRelocks() = runTest {
        val keys = Keys(value()); val factory = Factory()
        val controller = TrustController(keys, factory, "android", { now }, true)
        controller.restore(); assertEquals(TrustState.PinRequired, controller.state.value)
        controller.unlockPin(); assertEquals(TrustState.Ready, controller.state.value)
        controller.lockPin(); assertEquals(TrustState.PinRequired, controller.state.value)
        keys.value = null; controller.restore(); assertEquals(TrustState.Enrollment, controller.state.value)
    }
    @Test fun restoreLockedAndExpiredCredentials() = runTest {
        val keys = Keys(value()); val controller = TrustController(keys, Factory(), "desktop", { now }, false)
        keys.locked = true; controller.restore(); assertEquals(TrustState.WalletLocked, controller.state.value)
        keys.locked = false; controller.restore(); assertEquals(TrustState.Ready, controller.state.value)
        keys.value = value().withSession("token", "2020-01-01T00:00:00Z")
        controller.restore(); assertEquals(TrustState.SessionExpired, controller.state.value)
        keys.value = value(expiry = "2020-01-01T00:00:00Z")
        controller.restore(); assertEquals(TrustState.ReEnroll, controller.state.value)
    }
    @Test fun loginAndResetHaveNoBearerAndInvalidPasswordIsNonterminal() = runTest {
        val keys = Keys(value(null)); val factory = Factory(); val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore(); assertEquals(TrustState.Login, controller.state.value)
        factory.problem = ApiProblem("invalid-credentials", 401, "")
        assertFailsWith<ApiProblem> { controller.login("password") }; assertEquals(TrustState.Login, controller.state.value)
        factory.problem = null
        factory.result = WireJson.parseToJsonElement("""{"session":{"token":"replacement","expires_at":"2027-01-01T00:00:00Z"}}""").jsonObject
        controller.login("password"); assertNull(factory.session); assertEquals(TrustState.Ready, controller.state.value)
        assertEquals("replacement", keys.value?.session)
        controller.resetPassword("abcde fghjk", "newpassword"); assertNull(factory.session)
        assertEquals("ABCDEFGHJK", factory.body?.get("code")?.jsonPrimitive?.content)
    }
    @Test fun expiredSessionIsClearedAndLogoutRetrySucceeds() = runTest {
        val keys = Keys(value()); val factory = Factory(); val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore(); factory.problem = ApiProblem("session-invalid", 401, "")
        assertFailsWith<ApiProblem> { controller.ownDevices() }
        assertEquals(TrustState.SessionExpired, controller.state.value); assertNull(keys.value?.session)
        controller.logout(); assertEquals(TrustState.Login, controller.state.value)
        factory.problem = ApiProblem("device-revoked", 401, "")
        controller.revoke("self", "step"); assertEquals(TrustState.ReEnroll, controller.state.value); assertNull(keys.value)
    }
    @Test fun lockedWalletBeforeEnrollmentIsRecoverableWithoutHttp() = runTest {
        val keys = Keys(null); val factory = Factory(); val controller = TrustController(keys, factory, "desktop", { now }, false)
        keys.locked = true
        assertFailsWith<WalletUnavailable> { controller.enroll("porchlight://enroll?host=lan&token=token&ca=sha256:${"ab".repeat(32)}", "password") }
        assertEquals(TrustState.WalletLocked, controller.state.value)
        assertEquals("", factory.path)
        keys.locked = false; controller.restore(); assertEquals(TrustState.Enrollment, controller.state.value)
    }
    @Test fun enrollmentMatchesStrictWireContract() = runTest {
        val keys = Keys(null); val factory = Factory(); val controller = TrustController(keys, factory, "android", { now }, true)
        factory.result = WireJson.parseToJsonElement("""{"device_id":"self","certificate":"leaf","ca_certificate":"root","session":{"token":"session","expires_at":"2027-01-01T00:00:00Z"},"endpoints":["https://lan:8443"]}""").jsonObject
        controller.enroll("porchlight://enroll?host=lan&token=token&ca=sha256:${"ab".repeat(32)}", "password")
        assertEquals(setOf("token", "password", "csr", "platform"), factory.body?.keys)
        assertEquals("android", factory.body?.get("platform")?.jsonPrimitive?.content)
        assertEquals("ab".repeat(32), keys.pin); assertEquals(TrustState.PinRequired, controller.state.value)
    }
    @Test fun refreshSavesSlidingSessionAndRenewsFromSameStore() = runTest {
        val keys = Keys(value(expiry = "2026-10-20T00:00:00Z")); val factory = Factory()
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        factory.handler = { path -> WireJson.parseToJsonElement(when (path) {
            "/v1/me" -> """{"member_id":"member","session_expires_at":"2027-04-01T00:00:00Z"}"""
            "/v1/endpoints" -> """{"endpoints":["https://newlan:8443"]}"""
            else -> """{"certificate":"renewed","not_after":"2027-10-05T00:00:00Z"}"""
        }).jsonObject }
        controller.restore(); controller.refreshEndpointsAndRenew()
        assertEquals("2027-10-01T00:00:00Z", keys.value?.certificateExpiresAt)
        assertEquals(1, keys.csrCalls); assertEquals("renewed", keys.value?.certificate)
        assertEquals("2027-04-01T00:00:00Z", keys.value?.sessionExpiresAt)
        assertEquals(listOf("https://newlan:8443"), keys.value?.endpoints)
    }
    @Test fun ownDeviceWireDecodeIsTypedAndMalformedResponsesAreRedacted() = runTest {
        val keys = Keys(value())
        val factory = Factory()
        factory.handler = { path -> WireJson.parseToJsonElement(if (path == "/v1/me") """{"member_id":"member"}"""
            else """{"devices":[{"id":"d","member_id":"member","label":"Tablet","platform":"android","created_at":"2026-10-06T12:00:00Z","current":true,"revoked_at":null},{"id":"other","member_id":"someone-else","label":"Other","platform":"desktop","created_at":"2026-10-06T12:00:00Z","current":false}],"items":[],"next_cursor":null}""").jsonObject }
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore()
        assertEquals(listOf(OwnDevice("d", "Tablet", "android", true, false)), controller.ownDevices())
        factory.handler = { path -> WireJson.parseToJsonElement(if (path == "/v1/me") """{"member_id":"member"}"""
            else """{"devices":[{"label":"private"}]}""").jsonObject }
        val error = assertFailsWith<ApiProblem> { controller.ownDevices() }
        assertEquals("invalid-response", error.type)
        assertNull(error.cause)
        assertFalse(error.toString().contains("private"))
    }
    @Test fun revokedTransportRequiresEnrollmentAndSelfRevokeRetryClearsTrust() = runTest {
        val keys = Keys(value())
        val factory = Factory()
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        controller.restore()
        factory.problem = DeviceCertificateRevoked()
        assertFailsWith<DeviceCertificateRevoked> { controller.ownDevices() }
        assertEquals(TrustState.ReEnroll, controller.state.value)
        controller.revoke("self", "step", "same-key")
        assertNull(keys.value)
        assertEquals(TrustState.ReEnroll, controller.state.value)
    }
    @Test fun enrollmentRetryReusesExactCsrButNewAttemptDoesNot() = runTest {
        val keys = Keys(null)
        val factory = Factory()
        val controller = TrustController(keys, factory, "desktop", { now }, false)
        val uri = "porchlight://enroll?host=lan&token=token&ca=sha256:${"ab".repeat(32)}"
        factory.problem = IllegalStateException("Reply lost after write")
        repeat(2) { assertFailsWith<IllegalStateException> { controller.enroll(uri, "password") } }
        assertEquals(1, keys.csrCalls)
        assertFailsWith<IllegalStateException> { controller.enroll(uri.replace("token=token", "token=other"), "password") }
        assertEquals(2, keys.csrCalls)
        assertFailsWith<IllegalStateException> { controller.enroll(uri, "password") }
        assertEquals(2, keys.csrCalls, "A different URI must not destroy the original ambiguous attempt")
    }
}
