package app.porchlight.shared

import app.porchlight.shared.live.DeliveryRecord
import app.porchlight.shared.live.writeDesktopDelivery
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.test.*

class PrerolloutClientDataNoticeTest {
    @Test fun rejectionNoticePrecedesPublicationOfRetryableDelivery() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { reject = true }
        val published = CompletableDeferred<Boolean>()
        lateinit var runtime: AuditFinalClientCNotifyRuntime
        runtime = AuditFinalClientCNotifyRuntime(bus, deliveryWriter = { path, text ->
            val records = WireJson.decodeFromJsonElement<List<DeliveryRecord>>(
                WireJson.parseToJsonElement(text).jsonObject["records"] ?: JsonArray(emptyList()))
            val row = records.singleOrNull()
            if (bus.attempts.get() > 0 && row != null && !row.inFlight && !row.posted)
                published.complete(runtime.notice.value.orEmpty().contains("notifications unavailable"))
            writeDesktopDelivery(path, text)
        })
        runtime.use {
            it.start()
            it.install(local = true)
            assertTrue(withTimeout(5_000) { published.await() }, "A published retryable rejection must already expose its notice")
        }
    }
}
