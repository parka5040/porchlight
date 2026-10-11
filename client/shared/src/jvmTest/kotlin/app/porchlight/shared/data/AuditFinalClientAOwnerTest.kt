package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.porchlight.shared.notificationFixture
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAOwnerTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun trust(member: String = "member-a", device: String = "device-a", ca: String = "root") = DeviceTrust(device, "leaf", ca,
        "session", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z", member)
    private class Keys(var value: DeviceTrust?) : DeviceKeyStore {
        var attempt: EnrollmentAttempt? = null
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust; attempt = null }
        override suspend fun clear() { value = null }
        override suspend fun csr() = "CSR"
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        override suspend fun loadEnrollmentAttempt(targetBinding: String?) = attempt
        override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) { this.attempt = attempt }
    }
    private class Server : ApiTransport, TrustTransportFactory {
        var member = "member-a"
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
            "/v1/me" -> buildJsonObject { put("member_id", member) }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://example.invalid")))) }
            "/v1/enrollment/complete" -> WireJson.parseToJsonElement("""{"device_id":"device-b","certificate":"leaf","ca_certificate":"root","session":{"token":"session","expires_at":"2027-01-01T00:00:00Z"},"endpoints":["https://example.invalid"]}""").jsonObject
            else -> buildJsonObject {}
        }
    }
    private class Fixture(val now: Instant) {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-owner-")
        val file = root.resolve("cache.db").toFile()
        val artifacts = root.resolve("attachments").toFile()
        private val disk = FileAttachmentStore(artifacts)
        @Volatile var blockFiles = false
        val store = object : AttachmentStore by disk {
            override suspend fun clear() {
                check(!blockFiles) { "Private file cleanup unavailable" }
                disk.clear()
            }
        }
        @Volatile var blocked = false
        fun cache() = ReadCache({
            val delegate = openDesktopCache(file)
            object : SqlDriver by delegate {
                override fun execute(identifier: Int?, sql: String, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<Long> {
                    if (blocked && sql.trimStart().startsWith("DELETE", ignoreCase = true)) error("Cache clear unavailable")
                    return delegate.execute(identifier, sql, parameters, binders)
                }
            }
        }, { now }, null, store)
    }
    private fun bind(cache: ReadCache, keys: Keys, server: Server, scope: CoroutineScope): TrustController {
        val controller = TrustController(keys, server, "desktop", { now }, false)
        HouseholdData(cache, GeneratedCall.trusted(controller), { now }).bindTrust(controller, scope,
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false, resumeFromCursor = true)
        return controller
    }
    private suspend fun seed(cache: ReadCache, fixture: Fixture) {
        val row = attachmentFixture()
        cache.install(emptySnapshot(41).copy(items = listOf(itemRead(row.itemId)), lists = listOf(listFixture()),
            comments = listOf(commentFixture()), attachments = listOf(row), notifications = listOf(notificationFixture())))
        fixture.store.write("0-${row.id}-${row.version}-file", byteArrayOf(1))
        fixture.artifacts.resolve("${row.id}-view.txt").writeText("private viewer copy")
    }
    private fun empty(cache: ReadCache) {
        val view = cache.view.value
        assertTrue(view.items.isEmpty() && view.lists.isEmpty() && view.comments.isEmpty() && view.attachments.isEmpty() && view.notifications.isEmpty(),
            "No previous owner's rows may be published")
        assertNull(cache.sync.value, "Never resume another owner's cursor")
    }

    @Test fun continuedProcessFailedClearThenEnrollmentCannotPublishReady() = failedEnrollmentClear(restart = false)
    @Test fun restartAfterFailedEnrollmentClearCannotPublishPreviousOwner() = failedEnrollmentClear(restart = true)
    @Test fun restartAfterFilePurgeFailureKeepsTheCleanupObligation() = failedEnrollmentClear(restart = true, filesOnly = true)
    private fun failedEnrollmentClear(restart: Boolean, filesOnly: Boolean = false) = runBlocking {
        val fixture = Fixture(now); var cache = fixture.cache(); val server = Server(); val keys = Keys(trust())
        var scope = CoroutineScope(SupervisorJob() + Dispatchers.Default); var controller = bind(cache, keys, server, scope)
        try {
            controller.restore(); seed(cache, fixture)
            fixture.blocked = !filesOnly; fixture.blockFiles = filesOnly
            controller.onProblem(ApiProblem("device-revoked", 401, "Revoked"))
            controller.startReEnrollment()
            server.member = "member-b"
            runCatching { controller.enroll("porchlight://enroll?host=example.invalid&ca=sha256:${"ab".repeat(32)}&token=fixture", "password") }
            assertNotEquals(TrustState.Ready, controller.state.value, "A successful enrollment must wait for cache/file cleanup")
            empty(cache)
            assertNotNull(cache.storageNotice.value, "Failed cleanup must remain recoverable")
            assertEquals("device-b", keys.value?.deviceId, "The accepted replacement identity is durable for retry")
            // Ordinary initialization must not forget the failed clear, even before a process restart.
            runCatching { cache.initialize() }; empty(cache)
            if (restart) {
                scope.cancel(); controller.close(); cache.close()
                cache = fixture.cache(); scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
                controller = bind(cache, keys, server, scope)
                controller.restore()
                assertNotEquals(TrustState.Ready, controller.state.value); empty(cache)
            }
            fixture.blocked = false; fixture.blockFiles = false
            controller.restore()
            assertEquals(TrustState.Ready, controller.state.value)
            withTimeout(5_000) { while (keys.value?.memberId != "member-b") delay(10) }
            empty(cache); assertTrue(fixture.artifacts.listFiles()!!.isEmpty())
        } finally { scope.cancel(); controller.close(); cache.close(); fixture.root.toFile().deleteRecursively() }
    }

    @Test fun restartMismatchFailsClosedThenRetriesBeforeReady() = restart(missing = false, fail = true)
    @Test fun restartMissingOwnerPurgesBeforePublishing() = restart(missing = true, fail = false)
    @Test fun restartRejectsChangedMemberDeviceAndHousehold() = runBlocking {
        for (replacement in listOf(trust(member = "member-b"), trust(device = "device-b"), trust(ca = "cm9vdC1i"))) {
            restartWith(replacement, missing = false, fail = false)
        }
    }
    private fun restart(missing: Boolean, fail: Boolean) = runBlocking { restartWith(trust(member = "member-b"), missing, fail) }
    private suspend fun restartWith(replacement: DeviceTrust, missing: Boolean, fail: Boolean) {
        val fixture = Fixture(now); var cache = fixture.cache(); val server = Server(); val keys = Keys(trust())
        var scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        var controller: TrustController? = null
        try {
            if (!missing) { controller = bind(cache, keys, server, scope); controller.restore() }
            seed(cache, fixture)
            scope.cancel(); controller?.close(); cache.close()
            keys.value = replacement; server.member = replacement.memberId!!
            fixture.blocked = fail
            cache = fixture.cache(); scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            controller = bind(cache, keys, server, scope)
            controller.restore()
            empty(cache)
            if (fail) {
                assertNotEquals(TrustState.Ready, controller.state.value)
                assertNotNull(cache.storageNotice.value)
                fixture.blocked = false; controller.restore()
            }
            assertEquals(TrustState.Ready, controller.state.value)
            empty(cache); assertTrue(fixture.artifacts.listFiles()!!.isEmpty())
        } finally { scope.cancel(); controller?.close(); cache.close(); fixture.root.toFile().deleteRecursively() }
    }
}
