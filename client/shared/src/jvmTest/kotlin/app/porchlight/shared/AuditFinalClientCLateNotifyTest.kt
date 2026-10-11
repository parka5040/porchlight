package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import java.nio.file.*
import java.util.concurrent.*
import kotlin.test.*

class AuditFinalClientCLateNotifyTest {
    @Test fun notifyThatPostsBeforeTimeoutIsWithdrawnAfterLogoutAndLateReply() = lateReply("logout")
    @Test fun retiredRecordWithdrawsLateHandleWithoutAnEpochChange() = lateReply("retire")
    @Test fun sameIdentityRecordsLateHandleWithoutReposting() = lateReply("live")
    @Test fun lateOldHandleCannotReplaceNewIdentityRecordWithTheSameKey() = lateReply("replacement")
    private fun lateReply(ending: String) = runBlocking {
        val f = AuditFinalClientCFixture()
        val controller = f.controller(Dispatchers.Default)
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "late-notify-")
        val cache = desktopReadCache(dir.resolve("cache.db").toFile()) { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
        val bus = FakeSessionBus()
        val posted = CompletableDeferred<Unit>(); val reply = CompletableFuture<Long>()
        val withdrawn = CompletableDeferred<Unit>(); val submitted = java.util.concurrent.atomic.AtomicInteger()
        val delayed = object : DesktopSessionBus by bus, AsyncDesktopSessionBus {
            override fun notifyAsync(owner: String, replaces: Long, title: String, body: String, actions: List<String>): CompletableFuture<Long> {
                val number = submitted.incrementAndGet()
                val handle = bus.notify(owner, replaces, title, body, actions)
                posted.complete(Unit); return if (number == 1) reply else CompletableFuture.completedFuture(handle)
            }
            override fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>): Long =
                notifyAsync(owner, replaces, title, body, actions).get(2_000, TimeUnit.MILLISECONDS)
            override fun dismiss(owner: String, id: Long) { bus.dismiss(owner, id); withdrawn.complete(Unit) }
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val live = DesktopLive(data, dir, scope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
            sessionFactory = { delayed }, sleepBusFactory = { NoSleepBus })
        data.bindTrust(controller, scope, onIdentityEnded = { live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        try {
            controller.restore(); cache.install(emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
            withTimeout(5_000) { posted.await() }
            delay(2_200) // the daemon has posted; only its reply is late beyond the production wait
            assertEquals(1, submitted.get(), "A timed-out record must not be blindly re-sent")
            withTimeout(3_000) {
                when (ending) {
                    "logout" -> controller.logout()
                    "retire" -> cache.replaceNotifications(emptyList())
                    "replacement" -> {
                        f.saved = checkNotNull(f.saved).withIdentity("second", emptyList()); controller.restore()
                        cache.install(emptySnapshot(2).copy(notifications = listOf(notificationFixture())))
                        while (submitted.get() < 2) delay(10)
                    }
                }
            }
            reply.complete(1L)
            if (ending != "live") {
                assertEquals(true, withTimeoutOrNull(3_000) { withdrawn.await(); true }, "Late Notify handle must remain owned and be withdrawn after logout")
                assertEquals(listOf(1L), bus.closed.toList())
            }
            fun records() = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject["records"]?.jsonArray.orEmpty()
            if (ending == "live" || ending == "replacement") {
                val expected = if (ending == "live") 1L else 2L
                withTimeout(3_000) { while (records().singleOrNull()?.jsonObject?.get("posted")?.jsonPrimitive?.boolean != true) delay(10) }
                assertEquals(expected, records().single().jsonObject.getValue("handle").jsonPrimitive.long)
                assertEquals(expected.toInt(), submitted.get())
                if (ending == "live") assertTrue(bus.closed.isEmpty())
            } else {
                assertEquals(1, submitted.get())
                assertTrue(records().isEmpty(), "Late completion cannot resurrect a retired record")
            }
        } finally { reply.complete(1L); live.close(); scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
