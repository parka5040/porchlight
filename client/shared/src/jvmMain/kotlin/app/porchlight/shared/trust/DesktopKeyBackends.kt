package app.porchlight.shared.trust

import app.porchlight.shared.files.readPrivateDesktopFile
import app.porchlight.shared.files.writePrivateDesktopFile
import app.porchlight.shared.files.deletePrivateDesktopFile
import app.porchlight.shared.files.deletePrivateStagingLeftovers
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path

/** Explicit opt-in (tests, dev, the M5 gate harness): `PORCHLIGHT_KEY_STORE=file` forces the key file at enrollment. */
internal const val KEY_STORE_ENV = "PORCHLIGHT_KEY_STORE"
/** Owner-only folder inside Porchlight's data directory: the backend record and, for the file backend, the secret. */
internal const val KEY_DIRECTORY = "device-key"
/** Non-secret device setting: which backend holds this device's PKCS#12 password ([KeyProtection.id]). */
internal const val KEY_BACKEND_RECORD = "backend"
internal const val KEY_FILE_SECRET = "pkcs12-password"

/**
 * Where a desktop device's PKCS#12 password can live (owner decision 2026-10-07): Secret Service, or an owner-only
 * file when no provider exists. [select] runs only at enrollment, when nothing is recorded yet.
 */
internal class DesktopKeyBackends(
    val keyDirectory: Path,
    private val secretService: SecretWallet,
    private val file: SecretWallet,
    val select: () -> KeyProtection,
) {
    fun wallet(protection: KeyProtection): SecretWallet = when (protection) {
        KeyProtection.SecretService -> secretService
        KeyProtection.OwnerOnlyFile -> file
    }
    /** The recorded backend, or null when this device has not recorded one (not enrolled, or enrolled before deskkey). */
    fun recorded(): KeyProtection? {
        val value = readPrivateDesktopFile(keyDirectory, KEY_BACKEND_RECORD)?.decodeToString()?.trim() ?: return null
        // An unknown record (corruption, or a newer version's backend) is never guessed at.
        return KeyProtection.entries.find { it.id == value } ?: throw LocalTrustUnavailable()
    }
    fun record(protection: KeyProtection) {
        writePrivateDesktopFile(keyDirectory, KEY_BACKEND_RECORD, durable = true) { it.write(protection.id.encodeToByteArray()) }
    }
    /** Re-enrollment: forget the file secret and the record, so the next enrollment chooses again. */
    fun forget() {
        removeStagingLeftovers()
        deletePrivateDesktopFile(keyDirectory, KEY_FILE_SECRET)
        deletePrivateDesktopFile(keyDirectory, KEY_BACKEND_RECORD)
    }
    /** M1: staged copies of the secret a crash left behind (only `.staging-*-part` in the private key folder). */
    fun removeStagingLeftovers() = deletePrivateStagingLeftovers(keyDirectory)
    /** Existence only (never reads the secret); a missing or unusable folder simply has none. */
    fun hasFileSecret(): Boolean = Files.exists(keyDirectory.resolve(KEY_FILE_SECRET), LinkOption.NOFOLLOW_LINKS)

    companion object {
        /** Production: Secret Service when a provider exists (after one activation attempt), else the owner-only file. */
        fun system(
            directory: Path,
            override: String? = System.getenv(KEY_STORE_ENV),
            bus: SecretServiceBus = DbusSecretServiceBus.Session,
            wait: () -> Unit = secretServicePollWait,
            keyDirectory: Path = directory.resolve(KEY_DIRECTORY),
        ) = DesktopKeyBackends(
            keyDirectory,
            DesktopSecretWallet(DirectSecretService(DesktopSecretWallet.SERVICE, DesktopSecretWallet.ACCOUNT, bus,
                SECRET_SERVICE_PROMPT_TIMEOUT_MILLIS, wait)),
            OwnerOnlyKeyFile(keyDirectory),
        ) { selectKeyProtection(override, bus, wait) }

        /** A device whose Secret Service is [wallet] (unit tests, the controller's live wallet probe); enrollment never
         * selects the file here (a device already recorded as `file` still opens with it). */
        fun secretService(directory: Path, wallet: SecretWallet) =
            DesktopKeyBackends(directory.resolve(KEY_DIRECTORY), wallet, OwnerOnlyKeyFile(directory.resolve(KEY_DIRECTORY))) { KeyProtection.SecretService }
    }
}

/** Enrollment-time choice. Only the explicit `file` opt-in is accepted; any other value fails closed (typo safety). */
internal fun selectKeyProtection(override: String?, bus: SecretServiceActivationBus, wait: () -> Unit): KeyProtection =
    when (override?.trim()) {
        null, "" -> if (secretServiceProviderExists(bus, wait)) KeyProtection.SecretService else KeyProtection.OwnerOnlyFile
        "file" -> KeyProtection.OwnerOnlyFile
        else -> throw IllegalArgumentException("$KEY_STORE_ENV accepts only \"file\" (store the device key secret in an owner-only file at enrollment); unset it to use Secret Service when available")
    }

/**
 * The PKCS#12 password in an owner-only file (0600, created atomically and fsynced, in a 0700 folder; symlinks and
 * other users' files refused) for machines with no Secret Service provider. Its protection is POSIX file permissions:
 * keep the disk encrypted. A folder that cannot be made owner-only fails closed with PrivateDirectoryUnavailable.
 */
internal class OwnerOnlyKeyFile(private val directory: Path) : SecretWallet {
    override fun read(): String? = readPrivateDesktopFile(directory, KEY_FILE_SECRET)?.decodeToString()?.takeIf { it.isNotEmpty() }
    override fun write(value: String) {
        writePrivateDesktopFile(directory, KEY_FILE_SECRET, durable = true) { it.write(value.encodeToByteArray()) }
    }
}
