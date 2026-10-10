package app.porchlight.shared.data

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import kotlin.time.Instant

/** Rebuildable private byte cache. Filenames are local validated IDs, never original_name or a server path. */
class FileAttachmentStore(private val root: File) : AttachmentStore {
    private val mutex = Mutex()
    private fun file(key: String): File {
        require(key.matches(Regex("[a-zA-Z0-9_-]{1,180}")))
        check(root.exists() || root.mkdirs())
        root.setReadable(false, false); root.setWritable(false, false); root.setExecutable(false, false)
        root.setReadable(true, true); root.setWritable(true, true); root.setExecutable(true, true)
        return File(root, key)
    }
    override suspend fun read(key: String): ByteArray? = withContext(Dispatchers.IO) { mutex.withLock {
        val f = file(key)
        if (!f.isFile || f.length() > MAX_ATTACHMENT_BYTES || Files.isSymbolicLink(f.toPath())) null
        else f.readBytes().also { f.setLastModified(System.currentTimeMillis()) }
    } }
    override suspend fun write(key: String, bytes: ByteArray) = withContext(Dispatchers.IO) { mutex.withLock {
        require(bytes.size <= MAX_ATTACHMENT_BYTES)
        val target = file(key); val staging = File(root, "$key-part")
        try {
            staging.outputStream().use { it.write(bytes) }
            staging.setReadable(false, false); staging.setWritable(false, false)
            staging.setReadable(true, true); staging.setWritable(true, true)
            Files.move(staging.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
            Unit
        } finally { staging.delete() }
    } }
    override suspend fun clear() = withContext(Dispatchers.IO) { mutex.withLock { root.listFiles()?.forEach { if (it.isFile) check(it.delete()) }; Unit } }
    override suspend fun remove(key: String) = withContext(Dispatchers.IO) { mutex.withLock { val f = file(key); check(!f.exists() || f.delete()); Unit } }
    override suspend fun removeView(id: String) = withContext(Dispatchers.IO) { mutex.withLock {
        require(id.matches(Regex("[a-zA-Z0-9_-]+")))
        root.listFiles()?.filter { it.name.startsWith("$id-view.") && it.isFile }?.forEach { check(it.delete()) }; Unit
    } }
    override suspend fun cleanup(before: Instant) = withContext(Dispatchers.IO) { mutex.withLock {
        root.listFiles()?.filter { it.isFile && (it.lastModified() < before.toEpochMilliseconds() || it.name.endsWith("-part")) }?.forEach { it.delete() }; Unit
    } }
}
