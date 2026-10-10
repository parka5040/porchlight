package app.porchlight.shared.transport

import app.porchlight.shared.live.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import io.ktor.utils.io.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class StreamTimeoutTest {
    @Test fun lineReadAllowsSecondHeartbeatSchedulingMargin() = runTest {
        val channel = ByteChannel(autoFlush = true)
        val engine = MockEngine(MockEngineConfig().apply {
            dispatcher = StandardTestDispatcher(testScheduler)
            addHandler { respond(channel, HttpStatusCode.OK, headersOf("Content-Type", "text/event-stream")) }
        })
        val api = KtorApiTransport(HttpClient(engine), listOf("https://fixture.invalid"),
            HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        var received = false
        try {
            backgroundScope.launch {
                delay(2 * STREAM_HEARTBEAT_MS + 1)
                channel.writeStringUtf8(": heartbeat\n\n")
                channel.close()
            }
            assertFailsWith<ConnectFailure> { api.stream(0, "session") { received = true } }
            assertTrue(received, "line read must not race the second heartbeat at 240 seconds")
            assertEquals(2 * STREAM_HEARTBEAT_MS + 1, testScheduler.currentTime)
        } finally { api.close(); channel.cancel() }
    }
    @Test fun silentLineReadEndsAtSharedWatchdogDeadline() = runTest {
        val channel = ByteChannel(autoFlush = true)
        val engine = MockEngine(MockEngineConfig().apply {
            dispatcher = StandardTestDispatcher(testScheduler)
            addHandler { respond(channel, HttpStatusCode.OK, headersOf("Content-Type", "text/event-stream")) }
        })
        val api = KtorApiTransport(HttpClient(engine), listOf("https://fixture.invalid"),
            HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        try {
            assertFailsWith<ConnectFailure> { api.stream(0, "session") {} }
            assertEquals(STREAM_WATCHDOG_MS, testScheduler.currentTime)
        } finally { api.close(); channel.cancel() }
    }
}
