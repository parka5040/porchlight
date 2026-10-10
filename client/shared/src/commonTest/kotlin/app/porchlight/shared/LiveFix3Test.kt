package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.milliseconds

@OptIn(ExperimentalCoroutinesApi::class)
class LiveFix3Test {
    @Test fun safetyNetTimeoutReturnsRetrySignalButExternalCancellationPropagates() = runTest {
        assertFalse(liveSafetyNet(true, true, { awaitCancellation() }, { error("wrong branch") }))
        assertEquals(60_000L, testScheduler.currentTime)
        assertFalse(liveSafetyNet(true, false, { error("wrong branch") }, { awaitCancellation() }))
        assertTrue(liveSafetyNet(false, false, {}, {}))
        assertFailsWith<TimeoutCancellationException> {
            withTimeout(10) { liveSafetyNet(false, false, {}, { awaitCancellation() }) }
        }
    }
    @Test fun frozenConnectedStateAndRecentApiRequestCannotHideAStaleStream() = runTest {
        var wall = Instant.fromEpochMilliseconds(0)
        val live = LiveSync(StreamSource { _, receive -> receive(StreamFrame.Ready); awaitCancellation() },
            { 1 }, {}, {}, {}, { wall })
        assertFalse(live.hasRecentStreamFrame())
        backgroundScope.launch { live.connectOnce() }
        runCurrent()
        assertTrue(live.hasRecentStreamFrame())
        wall += (STREAM_WATCHDOG_MS - 1).milliseconds
        assertTrue(live.hasRecentStreamFrame())
        wall += 1.milliseconds // wall clock moves during suspend; monotonic watchdog hasn't run yet
        assertEquals(ConnectionState.Connected, live.state.value)
        live.successfulRequest()
        assertFalse(live.hasRecentStreamFrame())
        var refreshed = false
        assertTrue(liveSafetyNet(true, live.hasRecentStreamFrame(), { error("stale stream took shortcut") }, { refreshed = true }))
        assertTrue(refreshed)
    }
    @Test fun shellZoneFlowIgnoresCursorAndItemTicklesButPublishesZoneChanges() = runTest {
        val member = app.porchlight.shared.api.models.MemberRead(false, "self", "h", "Member", "member", "#123456", "M", "Europe/Helsinki", "manual", 1)
        val initial = CacheView(members = listOf(member), settings = emptySnapshot(1).memberSettings)
        val view = MutableStateFlow(initial)
        val zones = mutableListOf<String>()
        backgroundScope.launch { view.memberZoneChanges { "UTC" }.collect { zones += it } }
        runCurrent()
        view.value = initial.copy(sync = SyncState(2, Instant.fromEpochMilliseconds(0)))
        runCurrent()
        view.value = view.value.copy(items = listOf(itemRead("i")))
        runCurrent()
        assertEquals(listOf("Europe/Helsinki"), zones)
        view.value = view.value.copy(members = listOf(member.copy(tzid = "Pacific/Auckland")))
        runCurrent()
        assertEquals(listOf("Europe/Helsinki", "Pacific/Auckland"), zones)
    }
    @Test fun stableDeviceOwnerMigratesLegacyMemberSuffixWithoutRegeneration() {
        assertEquals("owner", actionIdentityOwner("device", "device:", "owner") { error("discovery reset") })
        assertEquals("owner", actionIdentityOwner("device", "device:member", "owner") { error("upgrade reset") })
        assertEquals("new", actionIdentityOwner("other", "device:member", "owner") { "new" })
    }
}
