package app.porchlight.shared.data

import app.porchlight.shared.files.*
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.File
import java.nio.file.*
import java.nio.file.attribute.FileTime
import kotlin.time.Instant

/** Rebuildable desktop cache: one initial inventory, bounded bytes, last-use LRU and amortized rescans. */
class FileAttachmentStore(private val root: File, private val byteBudget: Long = 512L * 1024 * 1024) : AttachmentStore {
    init { require(byteBudget > 0) }
    private val mutex = Mutex()
    private data class Entry(val size: Long, var used: Long)
    private val entries = linkedMapOf<String, Entry>()
    private var scannedAt: Long? = null
    private val activeStages = java.util.concurrent.ConcurrentHashMap.newKeySet<Path>()
    private fun path(key: String): Path {
        require(key.matches(Regex("[a-zA-Z0-9_-]{1,180}")))
        return ensurePrivateDesktopDirectory(root.toPath()).resolve(key)
    }
    private fun inventory(before: Long = Long.MIN_VALUE) {
        val dir = ensurePrivateDesktopDirectory(root.toPath())
        entries.clear()
        Files.newDirectoryStream(dir).use { files ->
            for (file in files) {
                if (file in activeStages) continue
                if (Files.isSymbolicLink(file)) { Files.delete(file); continue }
                if (!Files.isRegularFile(file, LinkOption.NOFOLLOW_LINKS)) continue
                val used = Files.getLastModifiedTime(file, LinkOption.NOFOLLOW_LINKS).toMillis()
                if (used < before || file.fileName.toString().endsWith("-part")) Files.delete(file)
                // A handed-off viewer may open its pathname after xdg-open returns. LRU owns originals only;
                // viewer aliases retain the existing age, identity and visibility cleanup authority.
                else if (!file.fileName.toString().contains("-view.")) entries[file.fileName.toString()] = Entry(Files.size(file), used)
            }
        }
        scannedAt = System.nanoTime()
        trim()
    }
    private fun trim(protect: String? = null) {
        var bytes = entries.values.sumOf { it.size }
        for ((name, entry) in entries.entries.sortedBy { it.value.used }) {
            if (bytes <= byteBudget) break
            if (name != protect) { deletePrivateDesktopFile(root.toPath(), name); entries.remove(name); bytes -= entry.size }
        }
    }
    private fun touch(key: String) {
        val used = System.currentTimeMillis()
        Files.setLastModifiedTime(path(key), FileTime.fromMillis(used))
        entries.remove(key)
        entries[key] = Entry(Files.size(path(key)), used)
    }
    override suspend fun read(key: String): ByteArray? = withContext(Dispatchers.IO) { mutex.withLock {
        val file = path(key)
        if (!Files.isRegularFile(file, LinkOption.NOFOLLOW_LINKS) || Files.size(file) > MAX_ATTACHMENT_BYTES) null
        else readPrivateDesktopFile(root.toPath(), key)?.also { touch(key) }
    } }
    override suspend fun content(key: String): AttachmentContent? = withContext(Dispatchers.IO) { mutex.withLock {
        if (scannedAt == null) inventory()
        val file = path(key)
        if (!Files.isRegularFile(file, LinkOption.NOFOLLOW_LINKS) || Files.size(file) > MAX_ATTACHMENT_BYTES) null
        else { privateDesktopFilePath(root.toPath(), key); touch(key); diskContent(file) }
    } }
    private fun diskContent(file: Path): AttachmentContent = object : AttachmentContent {
        override val privatePath: String get() = file.toAbsolutePath().toString()
        override suspend fun bytes(): ByteArray = withContext(Dispatchers.IO) {
            check(Files.size(file) <= MAX_ATTACHMENT_BYTES)
            readPrivateDesktopFile(root.toPath(), file.fileName.toString()) ?: error("Attachment removed")
        }
        override suspend fun chunks(consume: suspend (ByteArray) -> Unit) = withContext(Dispatchers.IO) {
            ensurePrivateDesktopDirectory(root.toPath())
            Files.newInputStream(file, LinkOption.NOFOLLOW_LINKS).use { input ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    currentCoroutineContext().ensureActive()
                    val count = input.read(buffer)
                    if (count < 0) break
                    total += count
                    check(total <= MAX_ATTACHMENT_BYTES)
                    consume(buffer.copyOf(count))
                }
            }
        }
    }
    override suspend fun write(key: String, bytes: ByteArray) = withContext(Dispatchers.IO) { mutex.withLock {
        require(bytes.size <= MAX_ATTACHMENT_BYTES && bytes.size <= byteBudget)
        path(key)
        if (scannedAt == null) inventory()
        writePrivateDesktopFile(root.toPath(), key, false) { it.write(bytes) }
        touch(key); trim(key)
    } }
    override suspend fun stage(key: String, receive: suspend (suspend (ByteArray) -> Unit) -> Unit): StagedAttachmentContent = withContext(Dispatchers.IO) {
        path(key)
        val staging = mutex.withLock { createPrivateDesktopStaging(root.toPath()).also { activeStages.add(it) } }
        try {
            var size = 0L
            Files.newOutputStream(staging, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { output ->
                receive { bytes ->
                    size += bytes.size
                    if (size > MAX_ATTACHMENT_BYTES || size > byteBudget) throw AttachmentTooLarge()
                    output.write(bytes)
                }
            }
            object : StagedAttachmentContent {
                private var current = staging
                override val privatePath: String get() = current.toAbsolutePath().toString()
                override suspend fun chunks(consume: suspend (ByteArray) -> Unit) = diskContent(current).chunks(consume)
                override suspend fun bytes() = diskContent(current).bytes()
                override suspend fun publish() = withContext(Dispatchers.IO) { mutex.withLock {
                    if (scannedAt == null) inventory()
                    val target = path(key)
                    Files.move(staging, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
                    activeStages.remove(staging); current = target
                    touch(key); trim(key)
                } }
                override suspend fun discard() { withContext(NonCancellable + Dispatchers.IO) { mutex.withLock {
                    Files.deleteIfExists(current); activeStages.remove(staging); entries.remove(key)
                } } }
            }
        } catch (e: Exception) {
            withContext(NonCancellable) { Files.deleteIfExists(staging); activeStages.remove(staging) }
            throw e
        }
    }
    override suspend fun clear() = withContext(Dispatchers.IO) { mutex.withLock {
        ensurePrivateDesktopDirectory(root.toPath())
        Files.newDirectoryStream(root.toPath()).use { files -> for (file in files) Files.deleteIfExists(file) }
        entries.clear(); scannedAt = null
    } }
    override suspend fun remove(key: String) = withContext(Dispatchers.IO) { mutex.withLock {
        path(key); deletePrivateDesktopFile(root.toPath(), key); entries.remove(key); Unit
    } }
    override suspend fun removeView(id: String) = withContext(Dispatchers.IO) { mutex.withLock {
        require(id.matches(Regex("[a-zA-Z0-9_-]+")))
        ensurePrivateDesktopDirectory(root.toPath())
        Files.newDirectoryStream(root.toPath(), "$id-view.*").use { files -> for (file in files) {
            Files.deleteIfExists(file); entries.remove(file.fileName.toString())
        } }
    } }
    override suspend fun cleanup(before: Instant) = withContext(Dispatchers.IO) { mutex.withLock {
        inventory(before.toEpochMilliseconds())
    } }
}
