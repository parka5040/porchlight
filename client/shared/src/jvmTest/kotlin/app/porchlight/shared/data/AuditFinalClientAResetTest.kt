package app.porchlight.shared.data

import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAResetTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Keys : DeviceKeyStore {
        var value: DeviceTrust? = DeviceTrust("self", "leaf", "root", "session", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z", "member-a")
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun clear() { value = null }
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private fun root() = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-reset-")

    @Test fun cursorAcknowledgmentCannotConsumeExternalReset() = resetThenOperation(false)
    @Test fun cleanupCannotConsumeExternalReset() = resetThenOperation(true)
    private fun resetThenOperation(cleanup: Boolean) = runBlocking {
        val dir = root(); val file = dir.resolve("cache.db").toFile()
        val cache = desktopReadCache(file) { now }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val snapshot = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>(); val heartbeat = CompletableDeferred<Unit>()
        val started = CompletableDeferred<Unit>(); val finish = CompletableDeferred<Unit>()
        val transport = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/snapshot" -> { snapshot.complete(Unit); release.await(); WireJson.encodeToJsonElement(emptySnapshot(2)).jsonObject }
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> buildJsonObject {}
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) {
                while (currentCoroutineContext().isActive) { receive(StreamFrame.Heartbeat); heartbeat.complete(Unit); delay(25) }
            }
        }
        val keys = Keys()
        val controller = TrustController(keys, transport, "desktop", { now }, false)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        try {
            cache.initialize(keys.value!!.cacheOwner())
            cache.install(emptySnapshot(1))
            data.bindTrust(controller, scope, resumeFromCursor = true); controller.restore()
            withTimeout(5_000) { heartbeat.await() }
            val old = async { cache.inSession { fence -> started.complete(Unit); finish.await(); runCatching { cache.install(fence, emptySnapshot(99)) } } }
            started.await()
            val external = openDesktopCache(file)
            try { PorchlightDb(external).cacheQueries.clearSync() } finally { external.close() }
            if (cleanup) cache.cleanup() else cache.acknowledgeCursor(5)
            cache.initialize()
            assertNotNull(withTimeoutOrNull(2_000) { snapshot.await(); true }, "Reset must resnapshot despite an intervening operation and healthy stream")
            finish.complete(Unit)
            assertTrue(old.await().exceptionOrNull() is CancellationException, "The held pre-reset request fence must be rejected")
            release.complete(Unit)
            withTimeout(5_000) { cache.sync.first { it?.cursor == 2L } }
            Unit
        } finally { finish.complete(Unit); release.complete(Unit); scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }

    @Test fun recreatedDatabaseStillAllowsLogoutAndMemberSwitchCleanup() = runBlocking {
        val dir = root(); val file = dir.resolve("cache.db").toFile(); val files = dir.resolve("attachments").toFile()
        val store = FileAttachmentStore(files); val cache = desktopReadCache(file, store) { now }
        val keys = Keys()
        val transport = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
        }
        val controller = TrustController(keys, transport, "desktop", { now }, false)
        controller.identityEnded = { cache.clear() }
        suspend fun seedThenRecreate() {
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            store.write("0-${row.id}-${row.version}-file", byteArrayOf(1))
            files.resolve("${row.id}-view.txt").writeText("private viewer copy")
            cache.initialize()
            assertTrue(file.delete()); openDesktopCache(file).close()
        }
        try {
            controller.restore(); seedThenRecreate()
            controller.logout()
            assertFalse(keys.value!!.logoutPending)
            assertTrue(cache.view.value.items.isEmpty()); assertTrue(files.listFiles()!!.isEmpty(), "Logout must purge real private files after recreation")
            seedThenRecreate()
            keys.value = keys.value!!.withIdentity("member-b", keys.value!!.endpoints)
            controller.restore()
            assertTrue(cache.view.value.items.isEmpty(), "Member switch clear must land after recreation")
            assertTrue(files.listFiles()!!.isEmpty())
        } finally { controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
