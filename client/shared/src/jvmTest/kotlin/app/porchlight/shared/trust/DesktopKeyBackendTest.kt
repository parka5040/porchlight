package app.porchlight.shared.trust

import app.porchlight.shared.files.PrivateDirectoryUnavailable
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import org.junit.Assume.assumeTrue
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import java.net.URI
import java.nio.file.FileSystems
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.attribute.PosixFilePermissions
import java.security.KeyPair
import kotlin.test.*
import kotlin.time.Clock

/**
 * Owner decision 2026-10-07 (deskkey): desktop key storage is desktop-agnostic. Secret Service (any provider) stays
 * primary and a locked collection gets the provider's own Unlock prompt; with no provider at all the PKCS#12 password
 * goes to an owner-only file. The backend chosen at enrollment is recorded and never silently switched.
 * No real D-Bus: [FakeSecretService] stands in for the session bus and org.freedesktop.secrets.
 */
class DesktopKeyBackendTest {
    private val ownerDir = PosixFilePermissions.fromString("rwx------")
    private val ownerFile = PosixFilePermissions.fromString("rw-------")
    private fun tempDir(prefix: String): Path {
        val base = Path.of(System.getProperty("porchlight.test.dir")).resolve("deskkey")
        Files.createDirectories(base)
        return Files.createTempDirectory(base, prefix)
    }
    private fun keys(dir: Path, bus: FakeSecretService, env: String? = null, keyDirectory: Path = dir.resolve(KEY_DIRECTORY)) =
        DesktopKeyStore(dir, DesktopKeyBackends.system(dir, env, bus, useKeyring = false, wait = {}, keyDirectory = keyDirectory)) { TestCertificates.date }
    private fun mode(path: Path) = Files.getPosixFilePermissions(path, LinkOption.NOFOLLOW_LINKS)
    private fun record(dir: Path) = Files.readString(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD))
    private fun fileSecret(dir: Path) = dir.resolve(KEY_DIRECTORY).resolve(KEY_FILE_SECRET)
    private fun controller(keys: DesktopKeyStore, factory: TrustTransportFactory = EnrollingFactory(keys)) =
        TrustController(keys, factory, "desktop", { Clock.System.now() }, false)

    /** Answers enrollment with a certificate for the store's own key, like the server would for its CSR. */
    private class EnrollingFactory(private val keys: DesktopKeyStore) : TrustTransportFactory {
        private val rootKey = TestCertificates.key()
        private val root = TestCertificates.cert(rootKey)
        val uri get() = "porchlight://enroll?host=lan&token=token&ca=sha256:${fingerprint(root)}"
        override fun enrollment(uri: EnrollUri): ApiTransport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?,
                                         idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/enrollment/complete", path)
                val leaf = TestCertificates.cert(KeyPair(keys.publicKey(), keys.privateKey()), rootKey, root, ca = false)
                return buildJsonObject {
                    put("device_id", "device"); put("certificate", pem(leaf)); put("ca_certificate", pem(root))
                    putJsonObject("session") { put("token", "session"); put("expires_at", "2030-01-01T00:00:00Z") }
                    putJsonArray("endpoints") { add("https://localhost:8443") }
                }
            }
        }
        override suspend fun authenticated(trust: DeviceTrust): ApiTransport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?,
                                         idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
        }
    }
    private suspend fun enroll(keys: DesktopKeyStore): TrustController {
        val factory = EnrollingFactory(keys)
        return controller(keys, factory).also { it.enroll(factory.uri, "password"); assertEquals(TrustState.Ready, it.state.value) }
    }

    // (a) No provider at all: enrollment keeps the password in an owner-only file that survives a restart.
    @Test fun noSecretServiceProviderEnrollsIntoOwnerOnlyKeyFile() = runTest {
        val dir = tempDir("no-provider-")
        try {
            val absent = FakeSecretService(running = false)
            enroll(keys(dir, absent))
            assertEquals(listOf(SECRET_SERVICE_NAME, KDE_SECRET_SERVICE_NAME), absent.starts, "exactly one activation attempt")
            assertEquals(0, absent.connections, "nothing reaches Secret Service")
            assertEquals("file", record(dir))
            assertEquals(ownerDir, mode(dir.resolve(KEY_DIRECTORY)))
            assertEquals(ownerFile, mode(fileSecret(dir)))
            assertEquals(ownerFile, mode(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)))
            assertTrue(Files.size(fileSecret(dir)) >= 32)
            assertFalse(String(Files.readAllBytes(dir.resolve("device.p12")), Charsets.ISO_8859_1).contains("session"))
            // Restart: a fresh process, still no provider.
            val reopened = keys(dir, FakeSecretService(running = false))
            val restarted = controller(reopened)
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals("session", reopened.load()?.session)
            assertEquals(KeyProtection.OwnerOnlyFile, reopened.keyProtection())
            assertEquals(KeyProtection.OwnerOnlyFile, restarted.keyProtection(), "Settings → Account and devices reads it here")
            assertEquals("Protected by file permissions; keep your disk encrypted", KeyProtection.OwnerOnlyFile.note)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun noSessionBusCountsAsNoProvider() = runTest {
        val dir = tempDir("no-bus-")
        try {
            val keys = keys(dir, FakeSecretService(noBus = true))
            keys.csr()
            assertEquals("file", record(dir))
            assertEquals(KeyProtection.OwnerOnlyFile, keys.keyProtection())
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun activatableProviderIsPreferredOverTheFile() = runTest {
        val dir = tempDir("activatable-")
        try {
            val bus = FakeSecretService(running = false, activatable = setOf(SECRET_SERVICE_NAME))
            enroll(keys(dir, bus))
            assertEquals("secret-service", record(dir))
            assertFalse(Files.exists(fileSecret(dir)))
            assertEquals(1, bus.items.size)
        } finally { dir.toFile().deleteRecursively() }
    }

    // (b) Provider present but locked: the provider's own prompt, then proceed; dismissal is the recoverable lock.
    @Test fun lockedCollectionRequestsUnlockAtEnrollmentAndStartup() = runTest {
        val dir = tempDir("locked-")
        try {
            val bus = FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = true))
            enroll(keys(dir, bus))
            assertEquals(1, bus.unlockRequests); assertEquals(1, bus.prompts)
            assertTrue(bus.promptTimeouts.all { it in 1..SECRET_SERVICE_PROMPT_TIMEOUT_MILLIS }, "bounded wait")
            assertEquals("secret-service", record(dir))
            assertFalse(Files.exists(fileSecret(dir)))
            // Startup with the wallet locked again: the prompt is requested instead of refusing.
            val startup = bus.copy(locked = true)
            val restarted = controller(keys(dir, startup))
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals(1, startup.prompts)
            // An Unlock that needs no prompt proceeds silently.
            val silent = bus.copy(locked = true, unlock = FakeSecretService.Unlock.Immediate)
            controller(keys(dir, silent)).also { it.restore(); assertEquals(TrustState.Ready, it.state.value) }
            assertEquals(1, silent.unlockRequests); assertEquals(0, silent.prompts)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun dismissedUnlockIsRecoverableWalletLockedWithoutRepromptOrFallback() = runTest {
        val dir = tempDir("dismissed-")
        try {
            val bus = FakeSecretService()
            enroll(keys(dir, bus))
            val dismissing = bus.copy(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
            val restarted = controller(keys(dir, dismissing))
            restarted.restore()
            assertEquals(TrustState.WalletLocked, restarted.state.value)
            assertEquals(1, dismissing.prompts, "a dismissal is final for this attempt: no second prompt")
            assertTrue(dismissing.starts.isEmpty(), "a dismissal is not a missing provider")
            assertFalse(Files.exists(fileSecret(dir)), "never a file fallback")
            // "Retry after unlock" prompts again; completing it restores the device.
            dismissing.unlock = FakeSecretService.Unlock.Prompt(completes = true)
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals(2, dismissing.prompts)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun dismissedUnlockAtEnrollmentWritesNothing() = runTest {
        val dir = tempDir("dismissed-enroll-")
        try {
            val bus = FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
            val keys = keys(dir, bus)
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            assertFailsWith<WalletUnavailable> { controller.enroll(factory.uri, "password") }
            assertEquals(TrustState.WalletLocked, controller.state.value)
            assertTrue(bus.items.isEmpty())
            assertFalse(Files.exists(dir.resolve("device.p12")))
            assertFalse(Files.exists(fileSecret(dir)))
        } finally { dir.toFile().deleteRecursively() }
    }

    // (c) The recorded backend wins, in both directions.
    @Test fun secretServiceDeviceNeverFallsBackToTheFileWhenItsProviderDisappears() = runTest {
        val dir = tempDir("ss-gone-")
        try {
            enroll(keys(dir, FakeSecretService()))
            val gone = FakeSecretService(running = false)
            val keys = keys(dir, gone)
            assertFailsWith<WalletUnavailable> { keys.load() }
            val restarted = controller(keys(dir, gone))
            restarted.restore()
            assertEquals(TrustState.WalletLocked, restarted.state.value)
            assertFalse(Files.exists(fileSecret(dir)), "the file store is never created for a Secret Service device")
            assertEquals("secret-service", record(dir))
            assertEquals(KeyProtection.SecretService, keys.keyProtection())
            assertTrue(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun fileDeviceKeepsTheFileWhenAProviderAppears() = runTest {
        val dir = tempDir("file-stays-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            val appeared = FakeSecretService()
            val restarted = controller(keys(dir, appeared))
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals(0, appeared.connections); assertEquals(0, appeared.ownerChecks); assertTrue(appeared.starts.isEmpty())
            assertTrue(appeared.items.isEmpty())
            assertEquals("file", record(dir))
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun reEnrollmentIsTheOnlyWayToChangeBackends() = runTest {
        val dir = tempDir("re-enroll-")
        try {
            val first = enroll(keys(dir, FakeSecretService(running = false)))
            first.startReEnrollment()
            assertFalse(Files.exists(dir.resolve("device.p12")))
            assertFalse(Files.exists(fileSecret(dir)), "the old file secret goes with the key")
            assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)))
            val provider = FakeSecretService()
            enroll(keys(dir, provider))
            assertEquals("secret-service", record(dir))
            assertEquals(1, provider.items.size)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun legacyDeviceWithoutRecordIsSecretService() = runTest {
        val dir = tempDir("legacy-")
        try {
            val wallet = FakeWallet()
            val legacy = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            legacy.csr()
            Files.delete(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)) // enrolled before backends were recorded
            val bus = FakeSecretService(running = false)
            val keys = keys(dir, bus)
            assertEquals(KeyProtection.SecretService, keys.keyProtection())
            assertFailsWith<WalletUnavailable> { keys.csr() }
            assertFalse(Files.exists(fileSecret(dir)))
        } finally { dir.toFile().deleteRecursively() }
    }

    // (d) Explicit opt-in override: only at enrollment, never over a recorded backend.
    @Test fun environmentOverrideForcesTheFileAtEnrollmentOnly() = runTest {
        val dir = tempDir("override-")
        val other = tempDir("override-ss-")
        try {
            val present = FakeSecretService()
            enroll(keys(dir, present, env = "file"))
            assertEquals("file", record(dir))
            assertEquals(0, present.connections); assertEquals(0, present.ownerChecks); assertTrue(present.starts.isEmpty())
            // A Secret Service device ignores the override later.
            val bus = FakeSecretService()
            enroll(keys(other, bus))
            val restarted = controller(keys(other, bus, env = "file"))
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals("secret-service", record(other))
            assertFalse(Files.exists(fileSecret(other)))
        } finally { dir.toFile().deleteRecursively(); other.toFile().deleteRecursively() }
    }

    @Test fun unknownOverrideValueFailsClosedBeforeWritingAnything() = runTest {
        val dir = tempDir("override-typo-")
        try {
            val keys = keys(dir, FakeSecretService(running = false), env = "files")
            val failure = assertFailsWith<IllegalArgumentException> { keys.csr() }
            assertTrue(KEY_STORE_ENV in failure.message.orEmpty())
            assertFalse(Files.exists(dir.resolve("device.p12")))
            assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY)))
        } finally { dir.toFile().deleteRecursively() }
    }

    // (e) Pending enrollment attempts live in the PKCS#12, so they follow the recorded backend's password.
    @Test fun pendingAttemptsFollowTheFileBackend() = runTest {
        val dir = tempDir("pending-file-")
        try {
            val keys = keys(dir, FakeSecretService(running = false))
            val attempt = EnrollmentAttempt("target", newTrustRequestKey(), keys.csr(), keys.enrollmentKeyAlias)
            keys.saveEnrollmentAttempt(attempt)
            val appeared = FakeSecretService()
            val reopened = keys(dir, appeared)
            val pending = assertNotNull(reopened.loadEnrollmentAttempt("target"))
            assertEquals(attempt.idempotencyKey, pending.idempotencyKey)
            assertEquals(attempt.csr, pending.csr)
            assertEquals(0, appeared.connections)
            val disk = String(Files.readAllBytes(dir.resolve("device.p12")), Charsets.ISO_8859_1)
            assertFalse(disk.contains(attempt.idempotencyKey)); assertFalse(disk.contains(attempt.csr))
            // Without its file secret the attempt is unreadable: no other store is consulted.
            Files.delete(fileSecret(dir))
            assertFailsWith<LocalTrustUnavailable> { keys(dir, appeared).loadEnrollmentAttempt("target") }
            assertEquals(0, appeared.connections)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun pendingAttemptsFollowTheSecretServiceBackend() = runTest {
        val dir = tempDir("pending-ss-")
        try {
            val bus = FakeSecretService()
            val keys = keys(dir, bus)
            val attempt = EnrollmentAttempt("target", newTrustRequestKey(), keys.csr(), keys.enrollmentKeyAlias)
            keys.saveEnrollmentAttempt(attempt)
            assertFalse(Files.exists(fileSecret(dir)))
            assertFailsWith<WalletUnavailable> { keys(dir, bus.copy(running = false)).loadEnrollmentAttempt("target") }
            assertFailsWith<WalletUnavailable> { keys(dir, bus.copy(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))).loadEnrollmentAttempt("target") }
            assertEquals(attempt.idempotencyKey, keys(dir, bus.copy(locked = true)).loadEnrollmentAttempt("target")?.idempotencyKey)
            assertFalse(Files.exists(fileSecret(dir)))
        } finally { dir.toFile().deleteRecursively() }
    }

    // (f) The private key folder must be owner-only POSIX; anything else fails closed with the typed error.
    @Test fun symlinkedKeyFolderFailsClosed() = runTest {
        val dir = tempDir("symlink-")
        try {
            val elsewhere = Files.createDirectories(dir.resolve("elsewhere"))
            Files.createSymbolicLink(dir.resolve(KEY_DIRECTORY), elsewhere)
            val failure = assertFailsWith<PrivateDirectoryUnavailable> { keys(dir, FakeSecretService(running = false)).csr() }
            assertEquals(dir.resolve(KEY_DIRECTORY).toString(), failure.path)
            assertTrue("symbolic link" in failure.reason)
            Files.list(elsewhere).use { assertEquals(0L, it.count(), "nothing written through the link") }
            assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun symlinkedKeyFolderAtStartupIsAFixableStateNotLostTrust() = runTest {
        val dir = tempDir("symlink-restore-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            val moved = dir.resolve("moved")
            Files.move(dir.resolve(KEY_DIRECTORY), moved)
            Files.createSymbolicLink(dir.resolve(KEY_DIRECTORY), moved)
            val restarted = controller(keys(dir, FakeSecretService(running = false)))
            restarted.restore()
            val state = assertIs<TrustState.KeyStorageUnavailable>(restarted.state.value)
            assertEquals(dir.resolve(KEY_DIRECTORY).toString(), state.path)
            assertTrue(Files.exists(dir.resolve("device.p12")), "nothing is cleared")
            // Fixing the folder and retrying restores the device.
            Files.delete(dir.resolve(KEY_DIRECTORY)); Files.move(moved, dir.resolve(KEY_DIRECTORY))
            restarted.restore()
            assertEquals(TrustState.Ready, restarted.state.value)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun symlinkedKeyFileIsRefusedNotFollowed() = runTest {
        val dir = tempDir("symlink-file-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            val outside = dir.resolve("outside-secret")
            Files.move(fileSecret(dir), outside)
            Files.createSymbolicLink(fileSecret(dir), outside)
            val failure = assertFailsWith<PrivateDirectoryUnavailable> { keys(dir, FakeSecretService(running = false)).load() }
            assertTrue("symbolic link" in failure.reason, failure.reason)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun widenedKeyFileIsNarrowedBackToOwnerOnly() = runTest {
        val dir = tempDir("widened-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            Files.setPosixFilePermissions(fileSecret(dir), PosixFilePermissions.fromString("rw-r--r--"))
            Files.setPosixFilePermissions(dir.resolve(KEY_DIRECTORY), PosixFilePermissions.fromString("rwxr-xr-x"))
            assertEquals("session", keys(dir, FakeSecretService(running = false)).load()?.session)
            assertEquals(ownerFile, mode(fileSecret(dir)))
            assertEquals(ownerDir, mode(dir.resolve(KEY_DIRECTORY)))
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun foreignOwnedKeyFolderFailsClosed() = runTest {
        // A directory owned by root stands in for another user's folder; chmod on it fails for an ordinary user.
        val foreign = Path.of("/usr")
        val me = System.getProperty("user.name")
        assumeTrue("needs a non-root test user", me != "root" && Files.getAttribute(Path.of("/proc/self"), "unix:uid") != 0)
        assumeTrue("needs a foreign-owned directory", Files.isDirectory(foreign) && Files.getOwner(foreign).name != me)
        val dir = tempDir("foreign-")
        try {
            val failure = assertFailsWith<PrivateDirectoryUnavailable> {
                keys(dir, FakeSecretService(running = false), keyDirectory = foreign).csr()
            }
            assertEquals(foreign.toString(), failure.path)
            assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }

    // R85: Secret Service present but unusable while enrolling a device that has no key yet: an explicit, optional
    // choice of the owner-only file, recorded exactly like PORCHLIGHT_KEY_STORE=file. Never offered to an enrolled device.
    private fun unusableProviders() = listOf(
        "provider fails to start" to FakeSecretService(running = false, failingStart = true),
        "no default collection" to FakeSecretService(hasDefault = false),
        "unlock dismissed or timed out" to FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false)),
        "KDE compat activated but never claims the standard name" to
            FakeSecretService(running = false, activatable = setOf(KDE_SECRET_SERVICE_NAME), claimsStandardName = false),
        "session bus present but unusable" to FakeSecretService(brokenBus = true),
    )
    @Test fun everyUnusableProviderCaseOffersTheFileAtEnrollment() = runTest {
        for ((case, bus) in unusableProviders()) {
            val dir = tempDir("r85-offer-")
            try {
                val keys = keys(dir, bus)
                val factory = EnrollingFactory(keys)
                val controller = controller(keys, factory)
                assertFailsWith<WalletUnavailable>(case) { controller.enroll(factory.uri, "password") }
                assertEquals(TrustState.WalletLocked, controller.state.value, case)
                assertTrue(controller.fileKeyStoreOffered.value, case)
                assertFalse(Files.exists(dir.resolve("device.p12")), case)
                assertFalse(Files.exists(fileSecret(dir)), "$case: never a silent fallback")
                assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)), "$case: nothing recorded until chosen")
            } finally { dir.toFile().deleteRecursively() }
        }
    }

    // Fix round 1, I1: the owner's laptop only activates ksecretd as org.kde.secretservicecompat. If it never claims
    // org.freedesktop.secrets, that is an installed-but-unusable provider, never "no provider".
    @Test fun kdeCompatThatNeverClaimsTheStandardNameIsUnusableNotAbsent() = runTest {
        val dir = tempDir("i1-kde-late-")
        try {
            val bus = FakeSecretService(running = false, activatable = setOf(KDE_SECRET_SERVICE_NAME), claimsStandardName = false)
            val keys = keys(dir, bus)
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            assertFailsWith<WalletUnavailable> { controller.enroll(factory.uri, "password") }
            assertEquals(TrustState.WalletLocked, controller.state.value)
            assertTrue(controller.fileKeyStoreOffered.value, "the explicit R85 choice")
            assertTrue(KDE_SECRET_SERVICE_NAME in bus.starts)
            assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)), "never a silent file record")
            assertFalse(Files.exists(fileSecret(dir)))
            assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }

    // Fix round 1, M2: one user-visible attempt never shows a second unlock prompt.
    @Test fun anUnlockThatDoesNotUnlockPromptsOnlyOnce() = runTest {
        for ((case, prompt) in listOf(
            "completed but still locked" to FakeSecretService.Unlock.Prompt(completes = true, unlocks = false),
            "Prompt() call fails" to FakeSecretService.Unlock.Prompt(completes = true, throws = true),
        )) {
            val dir = tempDir("m2-")
            try {
                val bus = FakeSecretService()
                enroll(keys(dir, bus))
                val broken = bus.copy(locked = true, unlock = prompt)
                val restarted = controller(keys(dir, broken))
                restarted.restore()
                assertEquals(TrustState.WalletLocked, restarted.state.value, case)
                assertEquals(1, broken.prompts, case)
                // Enrollment of a fresh device: also one prompt, then the explicit file choice.
                val fresh = tempDir("m2-fresh-")
                try {
                    val freshBus = FakeSecretService(locked = true, unlock = prompt)
                    val keys = keys(fresh, freshBus)
                    val factory = EnrollingFactory(keys)
                    val controller = controller(keys, factory)
                    assertFailsWith<WalletUnavailable>(case) { controller.enroll(factory.uri, "password") }
                    assertEquals(1, freshBus.prompts, case)
                    assertTrue(controller.fileKeyStoreOffered.value, case)
                } finally { fresh.toFile().deleteRecursively() }
            } finally { dir.toFile().deleteRecursively() }
        }
    }

    // Fix round 1, M3: a completed CreateItem prompt is verified by reading the secret back before the key uses it.
    @Test fun createItemPromptMustReallyStoreTheSecret() = runTest {
        val dir = tempDir("m3-lost-")
        try {
            val lying = FakeSecretService(createPrompt = FakeSecretService.CreatePrompt(completes = true, stores = false))
            val keys = keys(dir, lying)
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            assertFailsWith<WalletUnavailable> { controller.enroll(factory.uri, "password") }
            assertEquals(1, lying.createPrompts)
            assertEquals(TrustState.WalletLocked, controller.state.value)
            assertTrue(controller.fileKeyStoreOffered.value)
            assertFalse(Files.exists(dir.resolve("device.p12")), "no key encrypted with a secret nobody kept")
            assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)))
        } finally { dir.toFile().deleteRecursively() }
        val ok = tempDir("m3-ok-")
        try {
            val honest = FakeSecretService(createPrompt = FakeSecretService.CreatePrompt(completes = true, stores = true))
            enroll(keys(ok, honest))
            assertEquals(1, honest.createPrompts)
            assertEquals("secret-service", record(ok))
            controller(keys(ok, honest.copy())).also { it.restore(); assertEquals(TrustState.Ready, it.state.value) }
        } finally { ok.toFile().deleteRecursively() }
    }

    // Fix round 1, M1: a crash between staging and rename leaves only owner-only `-part` files; they are removed.
    @Test fun stagedSecretLeftoversAreRemovedAtStartupAndOnReEnroll() = runTest {
        val dir = tempDir("m1-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            val keyDir = dir.resolve(KEY_DIRECTORY)
            fun leftover(name: String) = keyDir.resolve(name).also {
                Files.write(it, "stale".toByteArray()); Files.setPosixFilePermissions(it, ownerFile)
            }
            val first = leftover(".staging-123-part")
            val unrelated = keyDir.resolve("notes.txt").also { Files.write(it, "keep".toByteArray()) }
            controller(keys(dir, FakeSecretService(running = false))).also { it.restore(); assertEquals(TrustState.Ready, it.state.value) }
            assertFalse(Files.exists(first), "startup removes the staged copy")
            assertTrue(Files.exists(unrelated), "nothing else is touched")
            assertTrue(Files.exists(fileSecret(dir)))
            val second = leftover(".staging-456-part")
            val reEnroll = controller(keys(dir, FakeSecretService(running = false)))
            reEnroll.restore(); reEnroll.startReEnrollment()
            assertFalse(Files.exists(second), "re-enrollment removes it too")
            assertFalse(Files.exists(fileSecret(dir)))
        } finally { dir.toFile().deleteRecursively() }
    }

    // Fix round 1, M4: Start over deletes a key that never enrolled, so the R85 choice stays reachable.
    @Test fun startOverKeepsTheFileChoiceReachable() = runTest {
        val dir = tempDir("m4-")
        try {
            val bus = FakeSecretService()
            val keys = keys(dir, bus)
            keys.saveEnrollmentAttempt(EnrollmentAttempt("target", newTrustRequestKey(), keys.csr(), keys.enrollmentKeyAlias))
            val first = controller(keys)
            first.startOverEnrollment()
            assertFalse(Files.exists(dir.resolve("device.p12")), "no unused key left behind")
            // The wallet later becomes unusable: startup does not prompt for a device that has no key ...
            val dismissing = bus.copy(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
            val reopened = keys(dir, dismissing)
            val again = EnrollingFactory(reopened)
            val restarted = controller(reopened, again)
            restarted.restore()
            assertEquals(TrustState.Enrollment, restarted.state.value)
            assertEquals(0, dismissing.prompts)
            // ... and enrollment offers the explicit file choice, which then enrolls.
            assertFailsWith<WalletUnavailable> { restarted.enroll(again.uri, "password") }
            assertTrue(restarted.fileKeyStoreOffered.value)
            restarted.storeKeyInFileForEnrollment()
            restarted.enroll(again.uri, "password")
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals("file", record(dir))
        } finally { dir.toFile().deleteRecursively() }
    }

    // Fix round 1, M8: a file device that lost its record is not mistaken for a legacy Secret Service device.
    @Test fun fileDeviceWithoutItsRecordIsLostTrustNotSecretService() = runTest {
        val dir = tempDir("m8-")
        try {
            enroll(keys(dir, FakeSecretService(running = false)))
            Files.delete(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD))
            val provider = FakeSecretService(locked = true)
            val restarted = controller(keys(dir, provider))
            restarted.restore()
            assertEquals(TrustState.LocalTrustLost(false), restarted.state.value)
            assertEquals(0, provider.connections, "no Secret Service prompt for a file device")
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun absentProviderNeedsNoChoice() = runTest {
        val dir = tempDir("r85-absent-")
        try {
            val keys = keys(dir, FakeSecretService(running = false))
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            controller.enroll(factory.uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertFalse(controller.fileKeyStoreOffered.value)
            assertEquals("file", record(dir))
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun choosingTheFileEnrollsWithItAndSurvivesRestart() = runTest {
        val dir = tempDir("r85-choose-")
        try {
            val bus = FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
            val keys = keys(dir, bus)
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            assertFailsWith<WalletUnavailable> { controller.enroll(factory.uri, "password") }
            controller.storeKeyInFileForEnrollment()
            assertEquals(TrustState.Enrollment, controller.state.value)
            assertFalse(controller.fileKeyStoreOffered.value)
            assertEquals("file", record(dir), "recorded exactly like the env override")
            assertEquals(KeyProtection.OwnerOnlyFile, controller.keyProtection(), "the enrollment screen names the choice")
            // The choice survives a restart before enrolling; enrollment then never touches Secret Service.
            val stillLocked = bus.copy()
            val reopened = keys(dir, stillLocked)
            val again = EnrollingFactory(reopened)
            val restarted = controller(reopened, again)
            restarted.restore()
            assertEquals(TrustState.Enrollment, restarted.state.value)
            restarted.enroll(again.uri, "password")
            assertEquals(TrustState.Ready, restarted.state.value)
            assertEquals(0, stillLocked.connections)
            assertEquals(ownerFile, mode(fileSecret(dir)))
            assertTrue(bus.items.isEmpty())
            // And after enrollment: a restart restores from the file even once the wallet works again.
            val unlocked = bus.copy(locked = false)
            val later = controller(keys(dir, unlocked))
            later.restore()
            assertEquals(TrustState.Ready, later.state.value)
            assertEquals(0, unlocked.connections)
            assertEquals(KeyProtection.OwnerOnlyFile, later.keyProtection())
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun notChoosingKeepsTodaysUnlockAndRetry() = runTest {
        val dir = tempDir("r85-decline-")
        try {
            val bus = FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
            val keys = keys(dir, bus)
            val factory = EnrollingFactory(keys)
            val controller = controller(keys, factory)
            assertFailsWith<WalletUnavailable> { controller.enroll(factory.uri, "password") }
            assertTrue(controller.fileKeyStoreOffered.value)
            bus.unlock = FakeSecretService.Unlock.Prompt(completes = true)
            controller.restore() // "Retry after unlock"
            assertEquals(TrustState.Enrollment, controller.state.value)
            assertFalse(controller.fileKeyStoreOffered.value)
            controller.enroll(factory.uri, "password")
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals("secret-service", record(dir))
            assertFalse(Files.exists(fileSecret(dir)))
            assertEquals(1, bus.items.size)
        } finally { dir.toFile().deleteRecursively() }
    }

    @Test fun enrolledSecretServiceDeviceNeverSeesTheChoice() = runTest {
        val dir = tempDir("r85-enrolled-")
        try {
            val bus = FakeSecretService()
            enroll(keys(dir, bus))
            for ((case, broken) in listOf(
                "dismissed" to bus.copy(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false)),
                "provider gone" to bus.copy(running = false),
            )) {
                val keys = keys(dir, broken)
                val restarted = controller(keys)
                restarted.restore()
                assertEquals(TrustState.WalletLocked, restarted.state.value, case)
                assertFalse(restarted.fileKeyStoreOffered.value, case)
                assertFailsWith<IllegalStateException>(case) { restarted.storeKeyInFileForEnrollment() }
                assertFailsWith<IllegalStateException>(case) { keys.chooseFileKeyStore() }
                assertEquals("secret-service", record(dir), case)
                assertFalse(Files.exists(fileSecret(dir)), case)
            }
        } finally { dir.toFile().deleteRecursively() }
    }

    @OptIn(ExperimentalTestApi::class)
    @Test fun enrollmentGateOffersTheFileAndEnrollsWithIt() = runComposeUiTest {
        val dir = tempDir("r85-gate-")
        val bus = FakeSecretService(locked = true, unlock = FakeSecretService.Unlock.Prompt(completes = false))
        val keys = keys(dir, bus)
        val factory = EnrollingFactory(keys)
        val controller = controller(keys, factory)
        try {
            setContent { MaterialTheme { TrustGate(controller) { Text("Household content") } } }
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Enroll this device").fetchSemanticsNodes().isNotEmpty() }
            onNodeWithText("Protected by file permissions", substring = true).assertDoesNotExist()
            onNodeWithText("Enrollment URI").performTextInput(factory.uri)
            onNodeWithText("Password").performTextInput("password")
            onNodeWithText("Enroll device").performScrollTo().performClick()
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Store the key in a protected file instead").fetchSemanticsNodes().isNotEmpty() }
            onNodeWithText("Retry after unlock").assertExists()
            onNodeWithText("Protected by file permissions; keep your disk encrypted", substring = true).assertExists()
            onNodeWithText("Store the key in a protected file instead").performScrollTo().performClick()
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("owner-only key file", substring = true).fetchSemanticsNodes().isNotEmpty() }
            onNodeWithText("Enroll this device").assertExists()
            onNodeWithText("Enrollment URI").performTextInput(factory.uri)
            onNodeWithText("Password").performTextInput("password")
            onNodeWithText("Enroll device").performScrollTo().performClick()
            waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Household content").fetchSemanticsNodes().isNotEmpty() }
            assertEquals("file", record(dir))
            assertTrue(bus.items.isEmpty())
        } finally { controller.close(); dir.toFile().deleteRecursively() }
    }

    @Test fun keyFolderWithoutPosixPermissionsFailsClosed() = runTest {
        val dir = tempDir("non-posix-")
        try {
            // A ZIP filesystem has no POSIX owner-only permissions (like FAT or some network mounts).
            FileSystems.newFileSystem(URI.create("jar:" + dir.resolve("vfat.zip").toUri()), mapOf("create" to "true")).use { zip ->
                val keyDirectory = zip.getPath("/porchlight-key")
                val failure = assertFailsWith<PrivateDirectoryUnavailable> {
                    keys(dir, FakeSecretService(running = false), keyDirectory = keyDirectory).csr()
                }
                assertTrue("cannot keep files private" in failure.reason, failure.reason)
                assertFalse(Files.exists(keyDirectory.resolve(KEY_FILE_SECRET)))
            }
            assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }
}

/** In-memory org.freedesktop.secrets on a fake session bus: provider presence, activation, lock state and prompts. */
internal class FakeSecretService(
    var running: Boolean = true,
    private val activatable: Set<String> = emptySet(),
    var locked: Boolean = false,
    var unlock: Unlock = Unlock.Prompt(completes = true),
    private val noBus: Boolean = false,
    val items: MutableMap<Map<String, String>, String> = mutableMapOf(),
    /** A registered provider that cannot start (activation error other than "not activatable"). */
    private val failingStart: Boolean = false,
    private val hasDefault: Boolean = true,
    /** KDE's compat service acquires org.freedesktop.secrets when started (false: late or crashed, I1). */
    private val claimsStandardName: Boolean = true,
    /** The session bus exists but cannot be used (refused, timed out, transport missing; M5). */
    private val brokenBus: Boolean = false,
    /** CreateItem answers with a prompt (M3): null = no prompt. */
    private val createPrompt: CreatePrompt? = null,
) : SecretServiceBus {
    sealed interface Unlock {
        data object Immediate : Unlock
        /** [unlocks] false: the prompt reports success but the collection stays locked; [throws]: Prompt() itself fails. */
        data class Prompt(val completes: Boolean, val unlocks: Boolean = completes, val throws: Boolean = false) : Unlock
    }
    data class CreatePrompt(val completes: Boolean, val stores: Boolean)
    private var kdeRunning = false
    private var pendingCreate: Pair<Map<String, String>, String>? = null
    var createPrompts = 0
    val starts = mutableListOf<String>()
    val promptTimeouts = mutableListOf<Long>()
    var ownerChecks = 0; var connections = 0; var unlockRequests = 0; var prompts = 0
    /** Another process run against the same provider data (shared items). */
    fun copy(running: Boolean = this.running, locked: Boolean = this.locked, unlock: Unlock = this.unlock) =
        FakeSecretService(running, activatable, locked, unlock, noBus, items, failingStart, hasDefault, claimsStandardName, brokenBus, createPrompt)
    private fun bus() {
        if (noBus) throw SessionBusUnavailable(IllegalStateException("no session bus"))
        if (brokenBus) throw WalletUnavailable(IllegalStateException("session bus refused the connection"))
    }
    override fun hasOwner(name: String): Boolean {
        bus(); ownerChecks++
        return (name == SECRET_SERVICE_NAME && running) || (name == KDE_SECRET_SERVICE_NAME && kdeRunning)
    }
    override fun start(name: String): Boolean {
        bus(); starts += name
        if (failingStart) throw IllegalStateException("org.freedesktop.DBus.Error.Spawn.ChildExited")
        if (name !in activatable) return false
        if (name == KDE_SECRET_SERVICE_NAME) { kdeRunning = true; if (claimsStandardName) running = true } else running = true
        return true
    }
    override fun connect(): SecretServiceConnection { bus(); connections++; return Connection() }
    private inner class Connection : SecretServiceConnection {
        private val collection = "/org/freedesktop/secrets/collection/login"
        private fun provider() { if (!running) throw IllegalStateException("org.freedesktop.DBus.Error.ServiceUnknown") }
        override fun defaultCollection(): String? { provider(); return collection.takeIf { hasDefault } }
        override fun isLocked(collection: String): Boolean { provider(); return locked }
        override fun unlock(collection: String): SecretUnlock {
            provider(); unlockRequests++
            return when (unlock) {
                Unlock.Immediate -> { locked = false; SecretUnlock(listOf(collection), "/") }
                is Unlock.Prompt -> SecretUnlock(emptyList(), "/org/freedesktop/secrets/prompt/u1")
            }
        }
        override fun prompt(path: String, timeoutMillis: Long): Boolean {
            provider(); promptTimeouts += timeoutMillis
            if (path.endsWith("/c1")) {
                createPrompts++
                val answer = checkNotNull(createPrompt)
                val (attributes, value) = checkNotNull(pendingCreate)
                if (answer.completes && answer.stores) items[attributes] = value
                return answer.completes
            }
            prompts++
            val prompt = unlock as? Unlock.Prompt
            if (prompt?.throws == true) throw IllegalStateException("org.freedesktop.DBus.Error.NoReply")
            if (prompt?.unlocks == true) locked = false
            return prompt?.completes == true
        }
        override fun read(attributes: Map<String, String>): String? { provider(); check(!locked); return items[attributes] }
        override fun write(attributes: Map<String, String>, label: String, value: String): String {
            provider(); check(!locked)
            if (createPrompt != null && attributes !in items) { pendingCreate = attributes to value; return "/org/freedesktop/secrets/prompt/c1" }
            items[attributes] = value; return "/"
        }
        override fun delete(attributes: Map<String, String>): String { provider(); check(!locked); items.remove(attributes); return "/" }
        override fun close() {}
    }
}
