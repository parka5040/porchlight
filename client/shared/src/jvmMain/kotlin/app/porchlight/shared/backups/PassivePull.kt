package app.porchlight.shared.backups

import app.porchlight.shared.api.models.Bundle
import app.porchlight.shared.data.HouseholdLocked
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.nio.channels.FileChannel
import java.nio.ByteBuffer
import java.security.MessageDigest
import java.time.ZoneId
import kotlin.time.Instant

/** Encrypted-only storage. Filename identity is not trusted: final size and full SHA-256 authorize publication. */
class PassivePull(private val root: Path, private val source: BackupSource, private val now: () -> Instant,
    private val keep: Int = 8, private val zone: () -> ZoneId = { ZoneId.systemDefault() },
    private val jitter: () -> Double = { kotlin.random.Random.nextDouble(0.8, 1.2) }) : DesktopBackups {
    init { require(keep in 1..100) }
    private val mutex = Mutex()
    private val mutable = MutableStateFlow(BackupView())
    override val state: StateFlow<BackupView> = mutable
    private var failures = 0
    private var retryAt = 0L
    private var retryPending = false
    private var reachabilityFailed = false
    /** Test seam: fault injection at OwnedBackupFiles steps (e.g. "index.persist"). */
    internal var ownershipStep: (String) -> Unit = {}
    suspend fun resetView() = mutex.withLock { resetViewLocked() }
    private fun resetViewLocked() { mutable.value = BackupView(); failures = 0; retryAt = 0; retryPending = false; reachabilityFailed = false }
    private fun failed() {
        failures = (failures + 1).coerceAtMost(16)
        val base = (60L shl (failures - 1)).coerceAtMost(3600)
        val seconds = (base * jitter().coerceIn(0.8, 1.2)).toLong().coerceIn(1, 3600)
        retryAt = now().epochSeconds + seconds
    }
    private fun syncDirectory(dir: Path) { FileChannel.open(dir, StandardOpenOption.READ).use { it.force(true) } }
    suspend fun tick() = pullInternal(false)
    override suspend fun pull() = pullInternal(true)
    fun start(scope: CoroutineScope) = scope.launch {
        while (isActive) {
            try { tick() } catch (_: CancellationException) { currentCoroutineContext().ensureActive() }
            delay(60_000)
        }
    }
    private fun safe(path: Path) { require(!Files.isSymbolicLink(path)) { "Unsafe backup path" } }
    private fun directory(path: Path) {
        safe(path); Files.createDirectories(path); Files.setPosixFilePermissions(path, PosixFilePermissions.fromString("rwx------"))
    }
    private fun hash(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private fun digest(path: Path): String {
        safe(path)
        val digest = MessageDigest.getInstance("SHA-256")
        Files.newInputStream(path, LinkOption.NOFOLLOW_LINKS).use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) { val count = input.read(buffer); if (count < 0) break; digest.update(buffer, 0, count) }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }
    private fun write(path: Path, bytes: ByteArray) {
        safe(path)
        val tmp = Files.createTempFile(path.parent, ".pull-", ".tmp",
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------")))
        try {
            FileChannel.open(tmp, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use {
                val buffer = ByteBuffer.wrap(bytes); while (buffer.hasRemaining()) it.write(buffer); it.force(true)
            }
            Files.move(tmp, path, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            syncDirectory(path.parent)
        } finally { Files.deleteIfExists(tmp) } // never unlink a preexisting operator .tmp file
    }
    private fun verify(ok: Boolean, path: Path, reason: String,
        action: String = "check household server backup status, then refresh") {
        if (!ok) throw BackupStorageFailure("$reason at $path; $action")
    }
    private fun valid(bundle: Bundle, path: Path) {
        verify(bundle.propertySize > 0 && bundle.sha256.matches(Regex("[0-9a-f]{64}")), path, "Invalid backup metadata")
    }
    private suspend fun pullInternal(force: Boolean) = mutex.withLock {
        withContext(Dispatchers.IO) {
            var operationPath = root
            try {
                val household = try { source.authorizedHousehold() }
                    catch (e: ConnectFailure) { reachabilityFailed = true; throw e }
                if (household == null) { resetViewLocked(); return@withContext }
                // A recovered authorization probe ends its outage even on a non-due tick. Successful probes
                // alone do not reset backoff for an interrupted download or a storage failure.
                if (reachabilityFailed) { failures = 0; retryAt = 0; reachabilityFailed = false }
                mutable.value = mutable.value.copy(allowed = true, unreachableSince = null,
                    error = if (mutable.value.cantReachHome) null else mutable.value.error)
                if (!force && now().epochSeconds < retryAt) return@withContext
                directory(root)
                val dir = root.resolve(hash(household.toByteArray())); operationPath = dir; directory(dir)
                val lock = dir.resolve(".lock"); operationPath = lock; safe(lock)
                FileChannel.open(lock, StandardOpenOption.CREATE, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { channel ->
                    Files.setPosixFilePermissions(lock, PosixFilePermissions.fromString("rw-------"))
                    val held = try { channel.tryLock() } catch (_: java.nio.channels.OverlappingFileLockException) { null }
                    if (held == null) {
                        if (force) mutable.value = mutable.value.copy(error = "Another desktop process is pulling backups")
                        return@withContext
                    }
                    held.use {
                        operationPath = dir
                        val notices = mutableListOf<String>()
                        // A failed recovery/index check is retried with backoff even when no pull is due, so fixing
                        // the storage (or renaming a corrupt index aside) clears the error without a manual refresh.
                        val owned = try {
                            OwnedBackupFiles(dir, ownershipStep, notice = { message ->
                                notices += message
                                mutable.value = mutable.value.copy(notice = notices.joinToString("\n"))
                            })
                        } catch (e: Exception) { retryPending = true; throw e }
                        val due = dir.resolve(".next-pull"); operationPath = due; safe(due)
                        val next = if (Files.exists(due)) Files.readString(due).toLongOrNull() else null
                        mutable.value = mutable.value.copy(allowed = true)
                        if (!force && !retryPending && next != null && now().epochSeconds < next) return@withContext
                        retryPending = true
                        mutable.value = mutable.value.copy(allowed = true, pulling = true, error = null,
                            notice = notices.takeIf { it.isNotEmpty() }?.joinToString("\n"))
                        val status = source.status()
                        mutable.value = mutable.value.copy(status = status, unreachableSince = null)
                        val bundle = status.latest
                        if (bundle != null) {
                            valid(bundle, dir)
                            val stem = hash(bundle.id.toByteArray()) + "-" + bundle.sha256
                            val target = dir.resolve("$stem.tar.age"); safe(target)
                            val partial = dir.resolve("$stem.part"); safe(partial)
                            operationPath = partial
                            owned.writable(target); owned.writable(partial); owned.writable(dir.resolve("$stem.json"))
                            // Only explicitly recorded checkpoints may be discarded or resumed.
                            owned.paths().filter { it.fileName.toString().endsWith(".part") && it != partial }.forEach(owned::delete)
                            if (!Files.exists(target) || Files.size(target) != bundle.propertySize || digest(target) != bundle.sha256) {
                                owned.checkpoint(partial)
                                var offset = Files.size(partial)
                                // A full but invalid checkpoint cannot be resumed by length alone. Keep it until
                                // a validated 200 response replaces it; never delete useful bytes on a failed request.
                                if (offset >= bundle.propertySize) offset = 0
                                var received = offset
                                var unsynced = 0L
                                FileChannel.open(partial, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { out ->
                                    Files.setPosixFilePermissions(partial, PosixFilePermissions.fromString("rw-------")); out.position(offset)
                                    source.download(bundle, offset, { head ->
                                        verify(head.etag == "\"${bundle.sha256}\"", partial, "Backup changed")
                                        if (head.status == 200) {
                                            verify(head.length == bundle.propertySize, partial, "Invalid backup length")
                                            out.truncate(0); out.position(0); received = 0
                                        } else {
                                            verify(head.status == 206 && offset > 0 && head.length == bundle.propertySize - offset &&
                                                head.contentRange == "bytes $offset-${bundle.propertySize - 1}/${bundle.propertySize}", partial, "Invalid backup range")
                                        }
                                    }, { bytes ->
                                        verify(bytes.size.toLong() <= bundle.propertySize - received, partial, "Backup exceeds declared size")
                                        val buffer = ByteBuffer.wrap(bytes); while (buffer.hasRemaining()) out.write(buffer)
                                        received += bytes.size; unsynced += bytes.size
                                        if (unsynced >= 4 * 1024 * 1024) { out.force(true); unsynced = 0 } // Final SHA also guards crash-truncated offsets.
                                    })
                                    out.force(true)
                                }
                                verify(received == bundle.propertySize, partial, "Incomplete backup")
                                verify(digest(partial) == bundle.sha256, partial, "Backup checksum mismatch",
                                    "preserve this checkpoint aside, check household server backup status, then refresh")
                                source.ensureAuthorized()
                                owned.publish(partial, target)
                                syncDirectory(dir)
                            }
                            source.ensureAuthorized()
                            operationPath = dir.resolve("$stem.json")
                            owned.write(operationPath, WireJson.encodeToString(bundle).toByteArray())
                            retain(dir, owned)
                            source.ensureAuthorized()
                            mutable.value = mutable.value.copy(localBundle = target.fileName.toString())
                        }
                        val local = java.time.Instant.ofEpochSecond(now().epochSeconds).atZone(zone())
                        var nextNight = local.toLocalDate().atTime(4, 0).atZone(zone())
                        if (!nextNight.isAfter(local)) nextNight = local.toLocalDate().plusDays(1).atTime(4, 0).atZone(zone())
                        operationPath = due
                        write(due, nextNight.toEpochSecond().toString().toByteArray())
                        mutable.value = mutable.value.copy(pulling = false, error = null)
                        failures = 0; retryAt = 0; retryPending = false
                    }
                }
            } catch (e: CancellationException) { resetViewLocked(); throw e }
            catch (_: HouseholdLocked) { resetViewLocked() }
            catch (_: ConnectFailure) { failed(); mutable.value = mutable.value.copy(pulling = false, error = "Can't reach home", unreachableSince = mutable.value.unreachableSince ?: now()) }
            catch (_: DeviceCertificateRevoked) { resetViewLocked() }
            catch (e: ApiProblem) {
                if (e.status in listOf(401, 403)) resetViewLocked()
                else { failed(); mutable.value = mutable.value.copy(pulling = false, error = "Backup request failed (HTTP ${e.status}); check the household server backup status, then refresh") }
            }
            catch (e: BackupStorageFailure) { failed(); mutable.value = mutable.value.copy(pulling = false, error = e.message) }
            catch (_: Exception) {
                failed(); mutable.value = mutable.value.copy(pulling = false,
                    error = "Backup validation or storage failed at $operationPath; check directory permissions and free space, then refresh. If it repeats, preserve this file aside and refresh")
            }
        }
    }
    private fun retain(dir: Path, owned: OwnedBackupFiles) {
        val entries = owned.paths().filter { it.fileName.toString().endsWith(".json") }.mapNotNull { path ->
            try {
                val bundle = WireJson.decodeFromString<Bundle>(Files.readString(path)); valid(bundle, path)
                val expected = hash(bundle.id.toByteArray()) + "-" + bundle.sha256 + ".json"
                if (path.fileName.toString() == expected) path to bundle else null
            } catch (_: Exception) { null }
        }.sortedWith(compareByDescending<Pair<Path, Bundle>> { it.second.finishedAt }.thenByDescending { it.second.id })
        for ((path, _) in entries.drop(keep)) {
            val target = path.resolveSibling(path.fileName.toString().removeSuffix(".json") + ".tar.age")
            owned.delete(target); owned.delete(path)
        }
        // Crash between rename and receipt publication: only recorded, still-owned orphans qualify.
        owned.paths().filter { it.fileName.toString().endsWith(".tar.age") }.forEach { target ->
            val receipt = dir.resolve(target.fileName.toString().removeSuffix(".tar.age") + ".json")
            if (!Files.exists(receipt, LinkOption.NOFOLLOW_LINKS)) owned.delete(target)
        }
        syncDirectory(dir)
    }
}
