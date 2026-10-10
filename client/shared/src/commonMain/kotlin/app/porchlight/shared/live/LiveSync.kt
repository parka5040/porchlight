package app.porchlight.shared.live

import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import kotlin.random.Random
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

sealed interface StreamFrame {
    data object Heartbeat : StreamFrame
    data object Ready : StreamFrame
    data object Resnapshot : StreamFrame
    data object Drain : StreamFrame
    data class Tickle(val seq: Long, val kind: String, val entityType: String, val entityId: String, val evicted: Boolean) : StreamFrame
}
class InvalidStream : Exception("Invalid household stream")
/** Bounded SSE line decoder. Never exposes wire data in an error or log. */
class SseParser {
    private var id: String? = null
    private var event = "message"
    private val data = StringBuilder()
    private var bytes = 0
    fun line(line: String): StreamFrame? {
        bytes += line.length
        if (bytes > 65536) throw InvalidStream()
        if (line.startsWith(":")) { bytes -= line.length; return StreamFrame.Heartbeat }
        if (line.isEmpty()) {
            val frame = try {
                if (data.isEmpty()) null else {
                    val value = WireJson.parseToJsonElement(data.toString()).jsonObject
                    when (event) {
                        "ready" -> { if ((value["cursor"]?.jsonPrimitive?.long ?: -1) < 0) throw InvalidStream(); StreamFrame.Ready }
                        "resnapshot" -> StreamFrame.Resnapshot
                        "drain" -> StreamFrame.Drain
                        else -> {
                            val seq = value.getValue("seq").jsonPrimitive.long
                            if (seq < 0 || id?.toLongOrNull() != seq || value.getValue("kind").jsonPrimitive.content != event) throw InvalidStream()
                            StreamFrame.Tickle(seq, event, value.getValue("entity_type").jsonPrimitive.content,
                                value.getValue("entity_id").jsonPrimitive.content, value["evicted"]?.jsonPrimitive?.boolean ?: false)
                        }
                    }
                }
            } catch (_: Exception) { throw InvalidStream() }
            id = null; event = "message"; data.clear(); bytes = 0
            return frame
        }
        val field = line.substringBefore(':')
        val value = line.substringAfter(':', "").removePrefix(" ")
        when (field) { "id" -> id = value; "event" -> event = value; "data" -> { if (data.isNotEmpty()) data.append('\n'); data.append(value) } }
        return null
    }
}
fun interface StreamSource { suspend fun collect(after: Long, receive: suspend (StreamFrame) -> Unit) }
enum class ConnectionState { Stopped, Connecting, Connected, Backoff }
fun reconnectDelay(attempt: Int, jitter: Double): Long =
    ((1000L shl attempt.coerceIn(0, 6)).coerceAtMost(60000) * (1 + jitter.coerceIn(0.0, 1.0) / 2)).toLong()
class Reachability(start: Instant) {
    var lastSuccess: Instant = start; private set
    fun success(at: Instant) { if (at > lastSuccess) lastSuccess = at }
    fun unreachableSince(at: Instant): Instant? = lastSuccess.takeIf { at - it >= 5.minutes }
}
// docs/specs/stream.md: production heartbeat is 120 seconds; tolerate two misses plus scheduling margin.
const val STREAM_HEARTBEAT_MS = 120_000L
const val STREAM_WATCHDOG_MS = 2 * STREAM_HEARTBEAT_MS + 10_000L
private class TerminalFrame : Exception()
/** Cursor advances only AFTER authoritative refetch/eviction commits. Crash before ack simply replays a read. */
class LiveSync(private val source: StreamSource, private val cursor: () -> Long?,
               private val apply: suspend (StreamFrame.Tickle) -> Unit,
               private val acknowledge: suspend (Long) -> Unit, private val resnapshot: suspend () -> Unit,
               private val now: () -> Instant, private val jitter: () -> Double = { Random.nextDouble() }) {
    private val mutableState = MutableStateFlow(ConnectionState.Stopped)
    val state = mutableState.asStateFlow()
    val reachability = Reachability(now())
    private val mutableUnreachable = MutableStateFlow<Instant?>(null)
    val unreachableSince = mutableUnreachable.asStateFlow()
    private val lastSuccess = MutableStateFlow(reachability.lastSuccess)
    private var lastFrameAt: Instant? = null
    fun hasRecentStreamFrame(): Boolean = lastFrameAt?.let {
        val elapsed = (now() - it).inWholeMilliseconds
        state.value == ConnectionState.Connected && elapsed in 0 until STREAM_WATCHDOG_MS
    } ?: false
    private var healthyFrame = false
    fun successfulRequest() { reachability.success(now()); lastSuccess.value = reachability.lastSuccess; mutableUnreachable.value = null }
    fun tick() { mutableUnreachable.value = reachability.unreachableSince(now()) }
    suspend fun connectOnce() {
        healthyFrame = false
        if (cursor() == null) resnapshot()
        mutableState.value = ConnectionState.Connecting
        try { coroutineScope {
            var frameCount = 0L
            val waiting = MutableStateFlow<Long?>(frameCount)
            val watchdog = launch {
                waiting.collectLatest { marker ->
                    if (marker != null) { delay(STREAM_WATCHDOG_MS); throw InvalidStream() }
                }
            }
            try { source.collect(cursor() ?: throw InvalidStream()) { frame ->
                // The source cannot read heartbeats during authoritative refetch/commit; its request timeout applies there.
                waiting.value = null
                lastFrameAt = now()
                successfulRequest(); mutableState.value = ConnectionState.Connected
                if (frame is StreamFrame.Tickle || frame == StreamFrame.Heartbeat) healthyFrame = true
                when (frame) {
                    StreamFrame.Resnapshot -> { resnapshot(); throw TerminalFrame() }
                    StreamFrame.Drain -> throw TerminalFrame()
                    is StreamFrame.Tickle -> if (frame.seq > (cursor() ?: -1)) { apply(frame); acknowledge(frame.seq) }
                    else -> Unit
                }
                waiting.value = ++frameCount
            } } finally { watchdog.cancel() }
        } } catch (_: TerminalFrame) { /* reconnect from snapshot/new cursor */ }
    }
    private val wake = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    fun networkChanged() { wake.trySend(Unit) }
    suspend fun run() = coroutineScope {
        val timer = launch { lastSuccess.collectLatest { success ->
            while (isActive) {
                val remaining = ((success + 5.minutes) - now()).inWholeMilliseconds
                if (remaining <= 0) { tick(); break }
                delay(remaining.coerceAtMost(60_000)) // re-evaluate wall time after suspend/clock changes
            }
        } }
        var attempt = 0
        try {
            while (isActive) {
                while (wake.tryReceive().isSuccess) { /* discard healthy/disabled network notifications */ }
                var networkReconnect = false
                try { coroutineScope {
                    val connection = async { connectOnce() }
                    val network = async { wake.receive() }
                    try {
                        kotlinx.coroutines.selects.select<Unit> {
                            connection.onAwait { }
                            network.onAwait { networkReconnect = true; connection.cancel() }
                        }
                    } finally { connection.cancel(); network.cancel() }
                } } catch (e: CancellationException) { throw e } catch (_: Exception) { /* redacted */ }
                if (healthyFrame) attempt = 0
                if (networkReconnect) continue // connectivity changes reconnect immediately, not after backoff
                mutableState.value = ConnectionState.Backoff
                withTimeoutOrNull(reconnectDelay(attempt++, jitter())) { wake.receive() }
            }
        } finally { timer.cancel(); mutableState.value = ConnectionState.Stopped }
    }
}

/** Periodic worker awaits a read even when stay-connected was selected. */
suspend fun liveSafetyNet(stayConnected: Boolean, connected: Boolean, unread: suspend () -> Unit, refresh: suspend () -> Unit): Boolean =
    withTimeoutOrNull(60_000) {
        if (stayConnected && connected) unread() else refresh()
        true
    } ?: false

