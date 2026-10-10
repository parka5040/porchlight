package app.porchlight.shared.backups

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import java.nio.file.*
import java.security.*
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Base64
import kotlin.time.Clock

internal fun requireSmokeRuntimePath(path: Path, runtime: Path) {
    require(path.normalize().startsWith(runtime.normalize()) && path.normalize() != runtime.normalize()) { "Smoke paths must be project-private runtime state" }
}

/** Controller-only headless acceptance entry point. Explicit porch dev-init credentials, never normal app storage.
 * Uses the production household-root-only factory and the real passive pull implementation; no UI/wallet needed. */
fun backupPullSmoke(device: Path, origin: String): Int = runBlocking {
    try {
        val runtime = Path.of("").toRealPath().resolve("var").toRealPath()
        val home = Path.of(System.getProperty("user.home")).toRealPath(); requireSmokeRuntimePath(home, runtime)
        val dir = device.toRealPath(); requireSmokeRuntimePath(dir, runtime)
        fun read(name: String): String { val path = dir.resolve(name); require(!Files.isSymbolicLink(path)); return Files.readString(path) }
        val cert = read("client.crt"); val ca = read("ca.crt"); val token = read("session.token").trim()
        val metadata = WireJson.parseToJsonElement(read("enrollment.json")).jsonObject
        val keyBytes = Base64.getMimeDecoder().decode(read("client.key").replace("-----BEGIN PRIVATE KEY-----", "").replace("-----END PRIVATE KEY-----", ""))
        val key = try { KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(keyBytes)) } finally { keyBytes.fill(0) }
        val trust = DeviceTrust(metadata.getValue("device_id").jsonPrimitive.content, cert, ca, token,
            metadata.getValue("expires").jsonPrimitive.content, orderedEndpoints(listOf(origin)), certificate(cert).notAfter.toInstant().toString())
        val keys = object : JavaDeviceKeyStore {
            override fun privateKey() = key
            override fun publicKey() = certificate(cert).publicKey
            override suspend fun load() = trust
            override suspend fun csr(): String = error("Smoke does not enroll")
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) = error("Smoke does not store keys")
            override suspend fun clear() = Unit
        }
        val transport = OkHttpTrustFactory(keys, ClientInfo(BuildInfo.VERSION)).authenticated(trust)
        try {
            val source = object : BackupSource {
                override suspend fun authorizedHousehold(): String? {
                    val me = WireJson.decodeFromJsonElement<MeResponse>(transport.request("GET", "/v1/me", session = token))
                    return me.householdId.takeIf { me.role == "super_admin" }
                }
                override suspend fun status() = WireJson.decodeFromJsonElement<BackupStatus>(transport.request("GET", "/v1/backups", session = token))
                override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) =
                    transport.download("/v1/backups/latest/download", token, if (offset > 0) "bytes=$offset-" else null, "\"${bundle.sha256}\"", begin, chunk)
            }
            val pull = PassivePull(home.resolve(".local/share/porchlight/backups"), source, { Clock.System.now() })
            pull.pull()
            val view = pull.state.value
            if (!view.allowed || view.localBundle == null || view.error != null) {
                println("FAIL desktop passive pull: ${view.error ?: if (!view.allowed) "super-admin session required" else "no published bundle"}")
                return@runBlocking 1
            }
            println("PASS desktop passive pull sha256")
            0
        } finally { transport.close() }
    } catch (e: Exception) {
        // Gate redirects stdout to its private pull.log; no raw exception messages/bodies/credentials.
        println("FAIL desktop passive pull: ${e.javaClass.simpleName}"); 1
    }
}
