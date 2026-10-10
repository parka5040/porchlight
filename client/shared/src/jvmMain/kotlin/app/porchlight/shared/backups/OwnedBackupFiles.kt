package app.porchlight.shared.backups

import app.porchlight.shared.api.models.Bundle
import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.jsonObject
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.charset.CharacterCodingException
import java.nio.file.*
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest
import java.time.Instant
import java.util.UUID

internal class BackupStorageFailure(message: String, cause: Exception? = null) : Exception(message, cause)

/** Caller holds the household process lock. Intent is durable before creation; old identities survive
 * until replacement is durable. Each instance resolves either side of a recorded rename before considering legacy
 * files, and writes the index only when an intent was pending. Failures are classified: a corrupt index is the only
 * case advised to be renamed aside; storage errors (disk full, permission denied, read-only) leave every file and
 * record exactly as they were and name the storage fix, so the next retry finishes the work.
 */
internal class OwnedBackupFiles(private val dir: Path, private val step: (String) -> Unit = {},
    private val notice: (String) -> Unit = {}) {
    @Serializable private data class Pending(val source: String? = null, val identity: String? = null)
    @Serializable private data class Index(val version: Int = 2, val entries: Map<String, String>, val pending: Map<String, Pending>)
    private val index = dir.resolve(".owned-files")
    private val grammar = Regex("[0-9a-f]{64}-[0-9a-f]{64}\\.(part|json|tar\\.age)")
    private val temporary = Regex("\\.pull-[0-9a-f-]{36}\\.tmp")
    private val entries = mutableMapOf<String, String>()
    private val pending = mutableMapOf<String, Pending>()
    private val permissions = PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------"))
    init {
        load()
        storage(dir) { reconcile() } // a failure here skips legacy handling, so our pending files are never renamed aside
        reconcileLegacy()
    }
    private fun load() {
        if (!Files.exists(index, LinkOption.NOFOLLOW_LINKS)) return
        val text = try {
            if (!Files.isRegularFile(index, LinkOption.NOFOLLOW_LINKS)) throw corruptIndex(null) // link, directory, ...
            step("index.read")
            Files.readString(index)
        } catch (e: CharacterCodingException) { throw corruptIndex(e) }
        catch (e: IOException) { throw failure(index, e, defaultAction) } // e.g. permission denied: not corruption
        try {
            if (WireJson.parseToJsonElement(text).jsonObject.containsKey("entries")) {
                val saved = WireJson.decodeFromString<Index>(text)
                require(saved.version == 2)
                entries.putAll(saved.entries); pending.putAll(saved.pending)
            } else entries.putAll(WireJson.decodeFromString<Map<String, String>>(text)) // v1 migration
            require(entries.keys.all(grammar::matches) && pending.keys.all(grammar::matches))
            require(pending.values.all { it.source == null || grammar.matches(it.source) || temporary.matches(it.source) })
        } catch (e: Exception) { throw corruptIndex(e) }
    }
    private val defaultAction get() = "check permissions and free space on $dir, then refresh"
    private fun corruptIndex(cause: Exception?) = BackupStorageFailure("Backup ownership index $index is corrupt; " +
        "quit Porchlight, rename it aside within $dir (do not delete it), then refresh", cause)
    /** Storage errors never advise touching the index: fixing the storage is enough for the next retry. */
    private fun failure(path: Path, e: Exception, fallback: String): BackupStorageFailure {
        val reason = ((e as? java.nio.file.FileSystemException)?.reason ?: e.message).orEmpty()
        val (problem, action) = when {
            e is java.nio.file.AccessDeniedException || "Permission denied" in reason || "Operation not permitted" in reason ->
                "permission denied" to "make $dir and its files readable and writable by your user"
            "Read-only file system" in reason -> "read-only file system" to "make the disk holding $dir writable"
            "No space left" in reason || "Disk quota exceeded" in reason -> "disk full" to "free space on the disk holding $dir"
            else -> return BackupStorageFailure("Backup storage problem at $path; $fallback", e)
        }
        return BackupStorageFailure("Backup storage problem at $path ($problem); $action, then refresh", e)
    }
    private fun <T> storage(path: Path, fallback: String = defaultAction, block: () -> T): T =
        try { block() } catch (e: Exception) {
            if (e is BackupStorageFailure) throw e
            throw failure(path, e, fallback)
        }
    private fun identity(path: Path): String? {
        if (!Files.exists(path, LinkOption.NOFOLLOW_LINKS)) return null
        val attrs = Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        if (!attrs.isRegularFile || attrs.isSymbolicLink) return null
        // Supported desktop storage must expose stable file keys and creation time (e.g. Linux ext4).
        return "${requireNotNull(attrs.fileKey()) { "File ownership unavailable" }}:${attrs.creationTime()}"
    }
    fun owns(path: Path): Boolean = entries[path.fileName.toString()]?.let { it == identity(path) } ?: false
    fun paths(): List<Path> = entries.keys.map(dir::resolve).filter(::owns)
    fun writable(path: Path) {
        require(path.parent == dir && grammar.matches(path.fileName.toString()))
        if (Files.exists(path, LinkOption.NOFOLLOW_LINKS) && !owns(path))
            throw BackupStorageFailure("Backup file $path changed during pull; quit other writers and refresh to preserve it aside")
    }
    private fun persist() {
        step("index.persist")
        atomicWrite(index, WireJson.encodeToString(Index(entries = entries.toMap(), pending = pending.toMap())).toByteArray())
    }
    /** Resolves recorded intents. With none pending nothing is written (idle minute polls never touch storage);
     * vanished entries are then dropped in memory only and persisted with the next durable update. */
    private fun reconcile() {
        val recovering = pending.isNotEmpty()
        for ((name, intent) in pending.toMap()) {
            val target = dir.resolve(name)
            if (intent.source == null) {
                // Durable CREATE_NEW intent: a surviving checkpoint belongs to this transaction.
                identity(target)?.let { entries[name] = it }
            } else {
                val source = dir.resolve(intent.source)
                if (!Files.exists(source, LinkOption.NOFOLLOW_LINKS) && intent.identity != null && identity(target) == intent.identity) {
                    entries[name] = intent.identity
                    entries.remove(intent.source)
                } else if (temporary.matches(intent.source) && Files.exists(source, LinkOption.NOFOLLOW_LINKS) &&
                    identity(source) != null && (intent.identity == null || identity(source) == intent.identity)) {
                    // Only our recorded-intent private temp, never a glob or a replaced file.
                    Files.delete(source)
                }
                // If source survives, the move did not happen: retain the old destination and checkpoint.
            }
            pending.remove(name)
        }
        entries.keys.toList().filter { !Files.exists(dir.resolve(it), LinkOption.NOFOLLOW_LINKS) }.forEach(entries::remove)
        if (recovering) { syncDirectory(); persist() } // temp deletions are durable before the intent is dropped
    }
    fun checkpoint(path: Path) = storage(path) {
        writable(path)
        if (!Files.exists(path, LinkOption.NOFOLLOW_LINKS)) {
            val name = path.fileName.toString()
            pending[name] = Pending(); persist(); step("checkpoint.intent")
            Files.createFile(path, permissions); step("checkpoint.create")
            syncDirectory()
            entries[name] = requireNotNull(identity(path)); pending.remove(name); persist()
            step("checkpoint.committed")
        }
    }
    fun publish(partial: Path, target: Path) = storage(target) {
        require(owns(partial)); writable(target)
        val name = target.fileName.toString()
        pending[name] = Pending(partial.fileName.toString(), requireNotNull(identity(partial)))
        persist(); step("publish.intent")
        Files.move(partial, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        step("publish.move"); syncDirectory()
        entries[name] = requireNotNull(identity(target)); entries.remove(partial.fileName.toString())
        pending.remove(name); persist(); step("publish.committed")
    }
    fun write(path: Path, bytes: ByteArray) = storage(path) {
        writable(path)
        val name = path.fileName.toString()
        val tmp = dir.resolve(".pull-${UUID.randomUUID()}.tmp")
        pending[name] = Pending(tmp.fileName.toString()); persist(); step("write.intent")
        Files.createFile(tmp, permissions); step("write.create")
        forceWrite(tmp, bytes); step("write.content")
        pending[name] = Pending(tmp.fileName.toString(), requireNotNull(identity(tmp)))
        persist(); step("write.ready")
        Files.move(tmp, path, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        step("write.move"); syncDirectory()
        entries[name] = requireNotNull(identity(path)); pending.remove(name); persist(); step("write.committed")
        // On failure leave the intent and surviving files for startup reconciliation, not speculative cleanup.
    }
    fun delete(path: Path) = storage(path) {
        if (owns(path)) {
            Files.delete(path); syncDirectory(); entries.remove(path.fileName.toString()); persist()
        }
    }
    private fun verifiedPair(receipt: Path, target: Path): Boolean = try {
        if (identity(receipt) == null || identity(target) == null) false else {
            val bundle = WireJson.decodeFromString<Bundle>(Files.readString(receipt))
            val stem = hash(bundle.id.toByteArray()) + "-" + bundle.sha256
            bundle.propertySize > 0 && bundle.sha256.matches(Regex("[0-9a-f]{64}")) &&
                receipt.fileName.toString() == "$stem.json" && target.fileName.toString() == "$stem.tar.age" &&
                Files.size(target) == bundle.propertySize && digest(target) == bundle.sha256
        }
    } catch (_: Exception) { false }
    private fun reconcileLegacy() {
        val candidates = storage(dir) { Files.list(dir).use { files -> files.filter { grammar.matches(it.fileName.toString()) && !owns(it) }.toList() } }
        val receipts = candidates.mapNotNull { path ->
            when {
                path.fileName.toString().endsWith(".json") -> path
                path.fileName.toString().endsWith(".tar.age") -> path.resolveSibling(path.fileName.toString().removeSuffix(".tar.age") + ".json")
                else -> null
            }
        }.distinct()
        for (path in receipts) {
            val target = path.resolveSibling(path.fileName.toString().removeSuffix(".json") + ".tar.age")
            if (verifiedPair(path, target)) storage(path) {
                entries[path.fileName.toString()] = requireNotNull(identity(path))
                entries[target.fileName.toString()] = requireNotNull(identity(target))
                persist() // adoption is durable before retention may delete either file
            }
        }
        for (path in candidates.filter { !owns(it) }) storage(path, "quit Porchlight, move this file aside within $dir, then refresh") {
            val saved = path.resolveSibling("${path.fileName}.legacy-${Instant.now().toString().replace(':', '-')}-${UUID.randomUUID()}")
            Files.move(path, saved, StandardCopyOption.ATOMIC_MOVE)
            notice("Preserved unverified backup file ${path.fileName} as $saved")
            syncDirectory(); entries.remove(path.fileName.toString()); persist()
        }
    }
    private fun hash(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private fun digest(path: Path): String {
        val digest = MessageDigest.getInstance("SHA-256")
        Files.newInputStream(path, LinkOption.NOFOLLOW_LINKS).use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) { val count = input.read(buffer); if (count < 0) break; digest.update(buffer, 0, count) }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }
    private fun syncDirectory() = FileChannel.open(dir, StandardOpenOption.READ).use { it.force(true) }
    private fun forceWrite(path: Path, bytes: ByteArray) {
        FileChannel.open(path, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { out ->
            val buffer = ByteBuffer.wrap(bytes); while (buffer.hasRemaining()) out.write(buffer); out.force(true)
        }
    }
    private fun atomicWrite(path: Path, bytes: ByteArray) {
        require(!Files.isSymbolicLink(path))
        val tmp = Files.createTempFile(dir, ".ownership-", ".tmp", permissions)
        try {
            forceWrite(tmp, bytes)
            Files.move(tmp, path, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            syncDirectory()
        } finally { Files.deleteIfExists(tmp) }
    }
}
