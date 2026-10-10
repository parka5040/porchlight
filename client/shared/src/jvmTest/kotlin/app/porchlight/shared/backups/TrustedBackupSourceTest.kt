package app.porchlight.shared.backups

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.HouseholdLocked
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.nio.file.*
import java.security.MessageDigest
import kotlin.test.*
import kotlin.time.Instant

class TrustedBackupSourceTest {
    private val now = Instant.parse("2026-10-06T05:00:00Z")
    private val bytes = "encrypted fixture".toByteArray()
    private val bundle = Bundle("one", "one.tar.age", bytes.size.toLong(), MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }, now, now, now)
    private class Keys(var trust: DeviceTrust) : DeviceKeyStore {
        override suspend fun load() = trust
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { this.trust = trust }
        override suspend fun clear() = Unit
        override suspend fun csr() = "unused"
        override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
    }
    private inner class Server(var role: String) : TrustTransportFactory, ApiTransport {
        val paths = mutableListOf<String>()
        var afterMe: suspend () -> Unit = {}
        var afterStatus: suspend () -> Unit = {}
        var duringDownload: suspend () -> Unit = {}
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
            paths += path
            assertEquals("session", session)
            return when (path) {
                "/v1/me" -> {
                    afterMe()
                    WireJson.parseToJsonElement("""{"member_id":"member","user_id":"user","username":"admin","display_name":"Admin","role":"$role","household_id":"household","device":{"id":"device","label":"Desktop","platform":"desktop"},"session_expires_at":"2027-10-01T00:00:00Z","connection":{"transport":"lan"}}""").jsonObject
                }
                "/v1/backups" -> { afterStatus(); WireJson.encodeToJsonElement(BackupStatus(latest = bundle)).jsonObject }
                else -> error("Unexpected path: $path")
            }
        }
        override suspend fun download(path: String, session: String?, range: String?, ifRange: String?, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
            paths += path; assertEquals("session", session); assertEquals("\"${bundle.sha256}\"", ifRange)
            begin(DownloadHead(200, ifRange, bytes.size.toLong(), null))
            chunk(bytes.copyOfRange(0, 2)); duringDownload(); chunk(bytes.copyOfRange(2, bytes.size))
        }
    }
    private suspend fun fixture(role: String, block: suspend (Path, Server, Keys, TrustController, TrustedBackupSource) -> Unit) {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "trusted-pull-")
        val keys = Keys(DeviceTrust("device", "leaf", "root", "session", "2027-10-01T00:00:00Z", listOf("https://lan"), "2027-10-01T00:00:00Z"))
        val server = Server(role); val controller = TrustController(keys, server, "desktop", { now }, false)
        try { controller.restore(); assertEquals(TrustState.Ready, controller.state.value); block(root, server, keys, controller, TrustedBackupSource(controller)) }
        finally { controller.close(); root.toFile().deleteRecursively() }
    }
    @Test fun realRoleGateRefusesAdminsAndMembersWithoutDiskMutation() = runTest {
        for (role in listOf("admin", "member")) fixture(role) { root, server, _, _, source ->
            PassivePull(root.resolve("not-created"), source, { now }).pull()
            assertEquals(listOf("/v1/me"), server.paths); assertFalse(Files.exists(root.resolve("not-created")))
            assertFailsWith<HouseholdLocked> { source.status() }
            assertEquals(listOf("/v1/me"), server.paths)
        }
    }
    @Test fun realSuperAdminPublishesVerifiedBytes() = runTest { fixture("super_admin") { root, server, _, _, source ->
        val pull = PassivePull(root, source, { now }); pull.pull()
        assertTrue(pull.state.value.allowed); assertNotNull(pull.state.value.localBundle)
        assertContentEquals(bytes, root.toFile().walk().single { it.extension == "age" }.readBytes())
        assertEquals(listOf("/v1/me", "/v1/backups", "/v1/backups/latest/download"), server.paths)
    } }
    @Test fun lateMeAndStatusRepliesCannotCrossEpochFence() = runTest {
        for (path in listOf("me", "status")) fixture("super_admin") { root, server, _, controller, source ->
            val end: suspend () -> Unit = { controller.onTransportError(DeviceCertificateRevoked()) }
            if (path == "me") server.afterMe = end else server.afterStatus = end
            val pull = PassivePull(root, source, { now })
            assertFailsWith<CancellationException> { pull.pull() }
            assertFalse(pull.state.value.allowed); assertEquals(0, root.toFile().walk().count { it.extension == "age" || it.extension == "json" })
            assertFalse(server.paths.contains("/v1/backups/latest/download"))
        }
    }
    @Test fun streamEpochAndSameEpochTokenChangeNeverPublish() = runTest {
        for (tokenOnly in listOf(false, true)) fixture("super_admin") { root, server, keys, controller, source ->
            server.duringDownload = {
                val epoch = controller.sessionEpoch.value
                if (tokenOnly) { keys.trust = keys.trust.withSession("replacement", "2027-10-01T00:00:00Z"); controller.restore(); assertEquals(epoch, controller.sessionEpoch.value) }
                else { controller.onTransportError(DeviceCertificateRevoked()); assertTrue(controller.sessionEpoch.value > epoch) }
            }
            val pull = PassivePull(root, source, { now })
            assertFailsWith<CancellationException> { pull.pull() }
            assertFalse(pull.state.value.allowed); assertNull(pull.state.value.localBundle)
            assertEquals(0, root.toFile().walk().count { it.extension == "age" || it.extension == "json" })
        }
    }
}
