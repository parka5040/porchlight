package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.security.*
import java.security.spec.ECGenParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x509.BasicConstraints
import org.bouncycastle.asn1.x509.Extension
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder
import java.math.BigInteger
import java.util.Date
import java.util.Base64

interface SecretWallet {
    fun read(): String?
    fun write(value: String)
    /** One user-visible attempt (startup, enrollment, Retry): Secret Service shows at most one unlock prompt inside. */
    fun <T> attempt(block: () -> T): T = block()
}

/**
 * One encrypted PKCS#12 snapshot holds key, cert, metadata and pending enrollment attempts; rename commits all of them
 * together. Its random password lives in the backend recorded at enrollment (owner decision 2026-10-07): Secret
 * Service, or an owner-only file when no provider exists. The record is honoured forever after: no silent migration
 * or fallback in either direction; re-enrollment ([clear]) is the only way to change backends.
 */
class DesktopKeyStore internal constructor(
    private val directory: Path,
    private val backends: DesktopKeyBackends,
    private val now: () -> Date,
) : JavaDeviceKeyStore {
    /** Production: Secret Service when a provider exists at enrollment, else the owner-only key file. */
    constructor(directory: Path = defaultDirectory(), now: () -> Date = { Date() }) : this(directory, DesktopKeyBackends.system(directory), now)
    /** A Secret Service device whose wallet is [wallet] (tests and the controller's live wallet probe). */
    constructor(directory: Path, wallet: SecretWallet, now: () -> Date = { Date() }) : this(directory, DesktopKeyBackends.secretService(directory, wallet), now)
    private var store: KeyStore? = null
    private var password: CharArray? = null
    private val file get() = directory.resolve("device.p12")
    /**
     * Devices enrolled before backends were recorded always used Secret Service. A key file without a record is not
     * such a device but a file device that lost its record (M8): lost trust, never a Secret Service prompt.
     */
    private fun protection(snapshotExists: Boolean): KeyProtection? = backends.recorded() ?: when {
        !snapshotExists -> null
        backends.hasFileSecret() -> throw LocalTrustUnavailable()
        else -> KeyProtection.SecretService
    }
    /** Called only after the desktop application's single-instance lock is acquired, like every key-store access. */
    private fun removeSnapshotStaging() {
        if (!Files.exists(directory, LinkOption.NOFOLLOW_LINKS)) return
        val root = app.porchlight.shared.files.ensurePrivateDesktopDirectory(directory)
        Files.newDirectoryStream(root, ".device-*.p12").use { entries ->
            for (entry in entries) {
                // Unlink symlinks without following them; never recurse into an unexpected directory.
                check(!Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS)) { "Invalid device staging file" }
                Files.deleteIfExists(entry)
            }
        }
        java.nio.channels.FileChannel.open(root, StandardOpenOption.READ).use { it.force(true) }
    }
    private fun open(create: Boolean): KeyStore? {
        store?.let { return it }
        removeSnapshotStaging()
        backends.removeStagingLeftovers() // M1: a crash may have left a staged copy of the secret
        val exists = Files.exists(file)
        if (!exists && !create) return null
        val recorded = protection(exists)
        val secret = if (exists) backends.wallet(checkNotNull(recorded)).let { wallet -> wallet.attempt { wallet.read() } } ?: throw LocalTrustUnavailable()
            // R85: with no key yet, every Secret Service failure (start, no default collection, dismissed or timed-out
            // unlock) is an enrollment-time one, for which the gate may offer the owner-only file explicitly.
            else try { firstSecret(recorded) } catch (e: WalletUnavailable) { throw SecretServiceUnusableForEnrollment(e) }
        password = secret.toCharArray()
        val opened = KeyStore.getInstance("PKCS12")
        if (Files.exists(file)) Files.newInputStream(file).use { opened.load(it, password) } else opened.load(null, password)
        store = opened
        return opened
    }
    /** Enrollment: the recorded (R85 choice) or freshly selected backend for a device that has no key yet. */
    private fun firstSecret(recorded: KeyProtection?): String {
        val protection = recorded ?: backends.select()
        val wallet = backends.wallet(protection)
        val secret = wallet.attempt {
            wallet.read() ?: Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(32).also { SecureRandom().nextBytes(it) }).also {
                wallet.write(it)
                // M3: trust a store (including a completed CreateItem prompt) only once the secret reads back.
                if (wallet.read() != it) {
                    if (protection == KeyProtection.SecretService) throw WalletUnavailable(IllegalStateException("Secret Service did not keep the key secret"))
                    error("The key file did not keep the key secret")
                }
            }
        }
        // Recorded once the secret is durable; a crash before this only repeats the enrollment-time choice.
        if (recorded == null) backends.record(protection)
        return secret
    }
    private fun commit(snapshot: KeyStore) {
        Files.createDirectories(directory, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")))
        Files.setPosixFilePermissions(directory, PosixFilePermissions.fromString("rwx------"))
        val temporary = Files.createTempFile(directory, ".device-", ".p12", PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")))
        try {
            Files.newOutputStream(temporary).use { snapshot.store(it, password) }
            java.nio.channels.FileChannel.open(temporary, StandardOpenOption.WRITE).use { it.force(true) }
            Files.move(temporary, file, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            java.nio.channels.FileChannel.open(directory, StandardOpenOption.READ).use { it.force(true) }
        } finally { Files.deleteIfExists(temporary) }
    }
    private fun ensureKey(): KeyStore {
        val opened = checkNotNull(open(true))
        if (!opened.containsAlias("device")) {
            val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
            // PKCS#12 requires a certificate even before enrollment. This local placeholder is never used for transport.
            val name = X500Name("CN=Unenrolled Porchlight device")
            val issuedAt = now().time
            val builder = JcaX509v3CertificateBuilder(name, BigInteger.ONE, Date(issuedAt - 60_000), Date(issuedAt + 86_400_000), name, pair.public)
            builder.addExtension(Extension.basicConstraints, true, BasicConstraints(false))
            val cert = JcaX509CertificateConverter().getCertificate(builder.build(JcaContentSignerBuilder("SHA256withECDSA").build(pair.private)))
            opened.setKeyEntry("device", pair.private, password, arrayOf(cert))
            commit(opened)
        }
        return opened
    }
    override fun publicKey() = ensureKey().getCertificate("device").publicKey
    override fun privateKey() = ensureKey().getKey("device", password) as PrivateKey
    override suspend fun csr(): String = withContext(Dispatchers.IO) { csr(publicKey(), privateKey()) }
    override suspend fun load(): DeviceTrust? = withContext(Dispatchers.IO) {
        val opened = open(false) ?: return@withContext null
        val key = opened.getKey("trust-metadata", password) ?: return@withContext null
        WireJson.decodeFromString<DeviceTrust>(key.encoded.toString(Charsets.UTF_8))
    }
    private fun cloneSnapshot(): KeyStore {
        val bytes = java.io.ByteArrayOutputStream().also { ensureKey().store(it, password) }.toByteArray()
        return KeyStore.getInstance("PKCS12").apply { load(bytes.inputStream(), password) }
    }
    private fun pendingEnrollments(): PendingEnrollments {
        val opened = open(false) ?: return PendingEnrollments()
        val entry = opened.getKey("enrollment-attempt", password) ?: return PendingEnrollments()
        if (!opened.containsAlias("device")) throw LocalTrustUnavailable()
        return WireJson.decodeFromString<PendingEnrollments>(entry.encoded.toString(Charsets.UTF_8))
    }
    override suspend fun loadEnrollmentAttempt(targetBinding: String?): EnrollmentAttempt? = withContext(Dispatchers.IO) {
        pendingEnrollments().find(targetBinding)?.also { check(it.keyAlias == "device") { "Enrollment key unavailable" } }
    }
    override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) = withContext(Dispatchers.IO) {
        check(attempt.keyAlias == "device")
        val pending = pendingEnrollments().with(attempt)
        val replacement = cloneSnapshot()
        replacement.setEntry("enrollment-attempt", KeyStore.SecretKeyEntry(SecretKeySpec(WireJson.encodeToString(pending).toByteArray(), "AES")), KeyStore.PasswordProtection(password))
        commit(replacement); store = replacement
    }
    override suspend fun clearEnrollmentAttempt(targetBinding: String?) = withContext(Dispatchers.IO) {
        if (open(false)?.containsAlias("enrollment-attempt") == true) {
            val remaining = pendingEnrollments().without(targetBinding)
            val replacement = cloneSnapshot()
            if (remaining.attempts.isEmpty()) replacement.deleteEntry("enrollment-attempt")
            else replacement.setEntry("enrollment-attempt", KeyStore.SecretKeyEntry(SecretKeySpec(WireJson.encodeToString(remaining).toByteArray(), "AES")), KeyStore.PasswordProtection(password))
            commit(replacement); store = replacement
        }
    }
    override suspend fun abandonEnrollment() = withContext(Dispatchers.IO) {
        val opened = open(false) ?: return@withContext
        if (!opened.containsAlias("trust-metadata")) {
            // M4: nothing enrolled, so nothing worth keeping: no empty key file that would make this device look
            // enrolled (and keep the R85 choice out of reach, or prompt at startup). The backend record stays.
            Files.deleteIfExists(file); store = null; password?.fill('\u0000'); password = null
            return@withContext
        }
        val replacement = cloneSnapshot()
        replacement.deleteEntry("enrollment-attempt")
        commit(replacement); store = replacement
    }
    override suspend fun save(trust: DeviceTrust, expectedPin: String?) = withContext(Dispatchers.IO) {
        validateTrust(trust, publicKey(), expectedPin)
        certificate(trust.certificate).checkValidity(now())
        // Clone first: a failed write must not mutate the in-memory committed snapshot.
        val replacement = cloneSnapshot()
        replacement.setKeyEntry("device", privateKey(), password, arrayOf(certificate(trust.certificate), certificate(trust.caCertificate)))
        replacement.setEntry("trust-metadata", KeyStore.SecretKeyEntry(SecretKeySpec(WireJson.encodeToString(trust).toByteArray(), "AES")), KeyStore.PasswordProtection(password))
        replacement.deleteEntry("enrollment-attempt")
        commit(replacement); store = replacement
    }
    override suspend fun clearSession(logoutPending: Boolean): DeviceTrust? = withContext(Dispatchers.IO) {
        val saved = load() ?: return@withContext null
        val cleared = saved.withSession(null, null).withLogoutPending(logoutPending)
        val replacement = cloneSnapshot()
        replacement.setEntry("trust-metadata", KeyStore.SecretKeyEntry(SecretKeySpec(WireJson.encodeToString(cleared).toByteArray(), "AES")), KeyStore.PasswordProtection(password))
        commit(replacement); store = replacement
        cleared
    }
    override suspend fun clear() = withContext(Dispatchers.IO) {
        removeSnapshotStaging()
        app.porchlight.shared.files.deletePrivateDesktopFile(directory, "device.p12")
        if (Files.exists(directory, LinkOption.NOFOLLOW_LINKS)) java.nio.channels.FileChannel.open(directory, StandardOpenOption.READ).use { it.force(true) }
        store = null; password?.fill('\u0000'); password = null
        // After the key is gone: the next enrollment chooses a backend afresh (re-enroll = the way to switch).
        backends.forget()
    }
    /** R85: the user's explicit enrollment-time choice, recorded exactly like PORCHLIGHT_KEY_STORE=file. Never for a
     * device that already has a key: an enrolled Secret Service device switches only by re-enrolling. */
    override suspend fun chooseFileKeyStore() = withContext(Dispatchers.IO) {
        check(store == null && !Files.exists(file)) { "This device already has a key; enroll again to change where it is kept" }
        backends.record(KeyProtection.OwnerOnlyFile)
    }
    /** For Settings: reads only the non-secret record, never Secret Service or the key secret. */
    override suspend fun keyProtection(): KeyProtection? = withContext(Dispatchers.IO) { protection(Files.exists(file)) }
    companion object {
        fun defaultDirectory(): Path {
            val data = System.getenv("XDG_DATA_HOME")?.takeIf { it.startsWith('/') } ?: "${System.getProperty("user.home")}/.local/share"
            return Path.of(data, "porchlight")
        }
    }
}
