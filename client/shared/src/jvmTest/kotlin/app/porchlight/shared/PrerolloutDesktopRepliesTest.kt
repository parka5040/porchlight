package app.porchlight.shared

import app.porchlight.shared.live.NativeDesktopSessionBus
import java.util.concurrent.*
import kotlin.test.*

class PrerolloutDesktopRepliesTest {
    @Test fun completingRepliesConcurrentlyWithCloseNeverBreaksDrain() {
        // Exercise only the production close bookkeeping, without opening thousands of native connections.
        val unsafe = sun.misc.Unsafe::class.java.getDeclaredField("theUnsafe").apply { isAccessible = true }.get(null) as sun.misc.Unsafe
        val type = NativeDesktopSessionBus::class.java
        val native = unsafe.allocateInstance(type)
        val pending = ConcurrentHashMap.newKeySet<CompletableFuture<Long>>()
        type.getDeclaredField("pending").apply { isAccessible = true }.set(native, pending)
        type.getDeclaredField("submissionLock").apply { isAccessible = true }.set(native, Any())
        val drain = type.getDeclaredMethod("abandonReplies").apply { isAccessible = true }
        val executor = Executors.newSingleThreadExecutor()
        try {
            repeat(200_000) {
                val replies = List(if (it % 2 == 0) 1 else 2) { CompletableFuture<Long>() }
                replies.forEach { reply -> pending.add(reply); reply.whenComplete { _, _ -> pending.remove(reply) } }
                val complete = executor.submit { replies.forEach { it.complete(42) } }
                drain.invoke(native)
                complete.get(2, TimeUnit.SECONDS)
                assertTrue(replies.all { it.isDone })
                assertTrue(pending.isEmpty())
            }
        } finally { executor.shutdownNow() }
    }
}
