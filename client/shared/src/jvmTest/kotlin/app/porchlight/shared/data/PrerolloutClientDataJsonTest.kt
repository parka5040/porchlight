package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.*
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.coroutines.CoroutineContext
import kotlin.test.*

class PrerolloutClientDataJsonTest {
    @Test fun injectedDispatcherHandlesModelEncodeDecodeAndTransportJson() = runBlocking {
        Executors.newSingleThreadExecutor { Thread(it, "fixture-json") }.asCoroutineDispatcher().use { json ->
            Executors.newSingleThreadExecutor { Thread(it, "fixture-ui") }.asCoroutineDispatcher().use { ui ->
                val dispatches = AtomicInteger()
                val counted = object : CoroutineDispatcher() {
                    override fun dispatch(context: CoroutineContext, block: Runnable) {
                        dispatches.incrementAndGet()
                        json.dispatch(context, block)
                    }
                }
                val api = KtorApiTransport(HttpClient(MockEngine {
                    respond("{\"value\":\"decoded\"}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "key" }), { false }, jsonDispatcher = counted)
                val call = GeneratedCall({ api }, { "session" }, jsonDispatcher = counted)
                try {
                    withContext(ui + CoroutineName("fixture-context")) {
                        val caller = Thread.currentThread()
                        val body = call.encode(EncodedThread())
                        assertTrue(body?.get("thread")?.jsonPrimitive?.content?.startsWith("fixture-json") == true)
                        val response = call.request("POST", "/v1/items", emptyMap(), emptyMap(), body)
                        assertEquals("decoded", response["value"]?.jsonPrimitive?.content)
                        assertTrue(call.decode<EncodedThread>(response).thread.startsWith("fixture-json"))
                        assertSame(caller, Thread.currentThread())
                        assertEquals("fixture-context", currentCoroutineContext()[CoroutineName]?.name)
                    }
                    assertTrue(dispatches.get() >= 4, "Encode, stringify, parse and model decode must use the injected dispatcher")
                } finally { api.close() }
            }
        }
    }

    @Test fun encodingCannotAcquireAReplacementIdentity() = runBlocking {
        var epoch = 0L
        val sent = AtomicInteger()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                sent.incrementAndGet()
                return buildJsonObject {}
            }
        }
        val call = GeneratedCall({ api }, { "session-$epoch" }, epoch = { epoch })
        assertFailsWith<CancellationException> {
            val body = call.encode(EncodingSwitch { epoch++ })
            call.request("POST", "/v1/items", emptyMap(), emptyMap(), body)
        }
        assertEquals(0, sent.get(), "Encoding an old intent must not send it with replacement credentials")
    }

    @Test fun identityOrRelockDuringDecodeStillWithholdsReply() = runBlocking {
        Executors.newSingleThreadExecutor().asCoroutineDispatcher().use { json ->
            val relocked = AtomicBoolean()
            var epoch = 0L
            val call = GeneratedCall({ error("No network") }, { "session" },
                beforeCall = { if (relocked.get()) throw HouseholdLocked() }, epoch = { epoch }, jsonDispatcher = json)
            val headers = mapOf("Idempotency-Key" to "fixture-key")
            val locked = assertFailsWith<HouseholdLockedAfterReply> {
                call.decodeJson("POST", "/v1/items", headers) { relocked.set(true); "decoded" }
            }
            assertEquals("fixture-key", locked.idempotencyKey)
            relocked.set(false)
            assertFailsWith<CancellationException> {
                call.decodeJson("GET", "/v1/snapshot", emptyMap()) { epoch++; "old identity" }
            }
            Unit
        }
    }
}

@kotlinx.serialization.Serializable(with = EncodingSwitchSerializer::class)
private data class EncodingSwitch(val switch: () -> Unit)
private object EncodingSwitchSerializer : KSerializer<EncodingSwitch> {
    override val descriptor = PrimitiveSerialDescriptor("EncodingSwitch", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): EncodingSwitch = error("Not decoded")
    override fun serialize(encoder: Encoder, value: EncodingSwitch) {
        value.switch()
        (encoder as JsonEncoder).encodeJsonElement(buildJsonObject {})
    }
}

@kotlinx.serialization.Serializable(with = EncodedThreadSerializer::class)
private data class EncodedThread(val thread: String = "")
private object EncodedThreadSerializer : KSerializer<EncodedThread> {
    override val descriptor = PrimitiveSerialDescriptor("EncodedThread", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): EncodedThread {
        (decoder as JsonDecoder).decodeJsonElement()
        return EncodedThread(Thread.currentThread().name)
    }
    override fun serialize(encoder: Encoder, value: EncodedThread) {
        (encoder as JsonEncoder).encodeJsonElement(buildJsonObject { put("thread", Thread.currentThread().name) })
    }
}
