package app.porchlight.shared.live

import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.*
import kotlin.random.Random
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

sealed interface StreamFrame {
    data object Heartbeat : StreamFrame
    data object Ready : StreamFrame
    data class CaughtUp(val cursor: Long) : StreamFrame
    data object Resnapshot : StreamFrame
    data object Drain : StreamFrame
    data class ProjectionChanged(val reason: String = "projection_changed") : StreamFrame
    data class Tickle(val seq: Long, val kind: String, val entityType: String, val entityId: String, val evicted: Boolean,
                      val itemId: String? = null, val listId: String? = null) : StreamFrame
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
                        "caught_up" -> {
                            val cursor = value["cursor"]?.jsonPrimitive?.long ?: throw InvalidStream()
                            if (cursor < 0 || id != null) throw InvalidStream()
                            StreamFrame.CaughtUp(cursor)
                        }
                        "resnapshot" -> if (value["reason"]?.jsonPrimitive?.contentOrNull == "projection_changed")
                            StreamFrame.ProjectionChanged() else StreamFrame.Resnapshot
                        "drain" -> StreamFrame.Drain
                        else -> {
                            val seq = value.getValue("seq").jsonPrimitive.long
                            if (seq < 0 || id?.toLongOrNull() != seq || value.getValue("kind").jsonPrimitive.content != event) throw InvalidStream()
                            StreamFrame.Tickle(seq, event, value.getValue("entity_type").jsonPrimitive.content,
                                value.getValue("entity_id").jsonPrimitive.content, value["evicted"]?.jsonPrimitive?.boolean ?: false,
                                value["item_id"]?.jsonPrimitive?.contentOrNull, value["list_id"]?.jsonPrimitive?.contentOrNull)
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
               private val now: () -> Instant, private val jitter: () -> Double = { Random.nextDouble() },
               private val concurrentApply: () -> Boolean = { false }) {
    private val mutableState = MutableStateFlow(ConnectionState.Stopped)
    val state = mutableState.asStateFlow()
    val reachability = Reachability(now())
    private val mutableUnreachable = MutableStateFlow<Instant?>(null)
    val unreachableSince = mutableUnreachable.asStateFlow()
    private val lastSuccess = MutableStateFlow(reachability.lastSuccess)
    private var lastFrameAt: Instant? = null
    internal var progressBlocked: () -> Boolean = { false }
    fun hasRecentStreamFrame(): Boolean = !progressBlocked() && lastFrameAt?.let {
        val elapsed = (now() - it).inWholeMilliseconds
        state.value == ConnectionState.Connected && elapsed in 0 until STREAM_WATCHDOG_MS
    } ?: false
    private var healthyFrame = false
    fun successfulRequest() {
        if (progressBlocked()) { tick(); return }
        reachability.success(now()); lastSuccess.value = reachability.lastSuccess; mutableUnreachable.value = null
    }
    fun tick() {
        mutableUnreachable.value = if (progressBlocked()) reachability.lastSuccess else reachability.unreachableSince(now())
        if (progressBlocked() && mutableState.value == ConnectionState.Connected) mutableState.value = ConnectionState.Backoff
    }
    private val connectionLock = kotlinx.coroutines.sync.Mutex()
    suspend fun connectOnce() { connectionLock.withLock { connect(false) } }
    /** Close only after the delivered prefix before caught_up has committed, never at ready/heartbeat/EOF. */
    suspend fun catchUp(timeoutMs: Long = 60_000): Boolean = withTimeoutOrNull(timeoutMs) {
        // A stale foreground connection yields to this bounded replay. The same lock prevents two independent
        // prefix trackers from applying/acknowledging overlapping deliveries on one cache.
        networkChanged()
        connectionLock.withLock {
            try { connect(true) } finally { mutableState.value = ConnectionState.Stopped }
        }
    } ?: false
    private suspend fun connect(stopAtCaughtUp: Boolean): Boolean {
        var completed = false
        healthyFrame = false
        if (cursor() == null) {
            resnapshot()
            if (stopAtCaughtUp) return true
        }
        mutableState.value = ConnectionState.Connecting
        try { coroutineScope {
            var frameCount = 0L
            val pending = ArrayDeque<Pair<Long, CompletableDeferred<Unit>>>()
            val prefixLock = kotlinx.coroutines.sync.Mutex()
            val failure = CompletableDeferred<Exception>()
            val failures = launch { throw failure.await() }
            suspend fun awaitAcknowledgedPrefix() {
                prefixLock.withLock { pending.map { it.second } }.forEach { it.await() }
                // Application completion precedes acknowledge under this lock. Await both, including a cursor
                // transaction already in progress after its positions were removed from pending.
                prefixLock.withLock { if (failure.isCompleted) throw failure.await() }
            }
            suspend fun applyDelivered(frame: StreamFrame.Tickle) {
                // Bound delivered-but-unacknowledged positions, not numeric sequence gaps.
                prefixLock.withLock { pending.firstOrNull()?.second.takeIf { pending.size >= 500 } }?.await()
                val done = CompletableDeferred<Unit>()
                val admitted = prefixLock.withLock {
                    // Replay can duplicate a delivered position before its original replacement commits.
                    // Never repeat its eviction fences or put a second copy behind the same prefix hole.
                    if (frame.seq <= (cursor() ?: -1) || pending.any { it.first == frame.seq }) false
                    else { pending.addLast(frame.seq to done); true }
                }
                if (!admitted) return
                launch(start = CoroutineStart.UNDISPATCHED) {
                    try {
                        apply(frame)
                        prefixLock.lock()
                        try {
                            done.complete(Unit)
                            var through: Long? = null
                            while (pending.firstOrNull()?.second?.isCompleted == true &&
                                pending.first().second.isCancelled.not()) through = pending.removeFirst().first
                            through?.let { acknowledge(it); healthyFrame = true }
                        } finally { prefixLock.unlock() }
                    } catch (e: CancellationException) {
                        done.cancel(e)
                        // A fenced cursor commit can cancel after application completed and its positions were
                        // removed. The replay barrier must still observe that failed acknowledgement.
                        failure.complete(e)
                        throw e
                    }
                    catch (e: Exception) {
                        done.completeExceptionally(e)
                        // Other resources already admitted by this batch may still commit. Never ack through this hole.
                        failure.complete(e)
                    }
                }
            }
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
                successfulRequest(); mutableState.value = if (progressBlocked()) ConnectionState.Backoff else ConnectionState.Connected
                when (frame) {
                    is StreamFrame.CaughtUp -> if (stopAtCaughtUp) {
                        awaitAcknowledgedPrefix()
                        completed = true
                        throw TerminalFrame()
                    }
                    StreamFrame.Resnapshot, is StreamFrame.ProjectionChanged -> {
                        awaitAcknowledgedPrefix()
                        resnapshot(); healthyFrame = true; completed = stopAtCaughtUp; throw TerminalFrame()
                    }
                    StreamFrame.Drain -> throw TerminalFrame()
                    is StreamFrame.Tickle -> if (frame.seq > (cursor() ?: -1)) {
                        if (concurrentApply()) applyDelivered(frame) else { apply(frame); acknowledge(frame.seq); healthyFrame = true }
                    }
                    else -> Unit
                }
                waiting.value = ++frameCount
            }
                waiting.value = null // EOF may still have application-owned replacements to drain.
                awaitAcknowledgedPrefix()
            } finally { watchdog.cancel(); failures.cancel() }
        } } catch (_: TerminalFrame) { /* reconnect from snapshot/new cursor */ }
        return completed
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

