package app.porchlight.shared.update

import app.porchlight.shared.api.models.ReleaseFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.file.*
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest

internal val updateFileMode = PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------"))

/** Platform- or user-owned prefixes may be links (Android's /data/user/0 -> /data/data, /home -> /var/home, linked
 * dotfile folders). Canonicalize the longest existing prefix once, so the strict guard below covers the rest. */
internal fun realUpdateRoot(path: Path): Path {
    var existing = path.toAbsolutePath().normalize()
    val missing = ArrayDeque<Path>()
    while (!Files.exists(existing)) { missing.addFirst(existing.fileName); existing = existing.parent ?: break }
    return missing.fold(existing.toRealPath()) { base, part -> base.resolve(part) }
}
internal fun guardedUpdateDirectory(path: Path): Path {
    val normalized = path.toAbsolutePath().normalize()
    var ancestor: Path? = normalized
    while (ancestor != null) { require(!Files.isSymbolicLink(ancestor)) { "Unsafe update folder" }; ancestor = ancestor.parent }
    return privateUpdateDirectory(normalized)
}
internal fun updatePath(root: Path, name: String): Path {
    requireReleasePath(name)
    val path = root.resolve(name).normalize()
    require(path.startsWith(root) && !Files.isSymbolicLink(path)) { "Unsafe update path" }
    var directory = root
    name.split('/').dropLast(1).forEach { directory = guardedUpdateDirectory(directory.resolve(it)) }
    return path
}
internal fun removeUpdateTree(path: Path) {
    if (!Files.exists(path, LinkOption.NOFOLLOW_LINKS)) return
    Files.walk(path).use { entries -> entries.sorted(Comparator.reverseOrder()).forEach { Files.delete(it) } }
}
internal fun validateReleaseFiles(files: List<ReleaseFile>) {
    require(files.isNotEmpty() && files.size <= 512)
    require(files.map { it.path }.distinct().size == files.size)
    files.forEach {
        requireReleasePath(it.path)
        require(it.propertySize in 0..(2L * 1024 * 1024 * 1024) && it.sha256.matches(Regex("[0-9a-f]{64}")))
    }
    require(files.sumOf { it.propertySize } <= 4L * 1024 * 1024 * 1024)
}
internal fun digestHex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
internal fun fileDigest(path: Path): String {
    val digest = MessageDigest.getInstance("SHA-256")
    Files.newInputStream(path, LinkOption.NOFOLLOW_LINKS).use { input ->
        val buffer = ByteArray(64 * 1024)
        while (true) { val count = input.read(buffer); if (count < 0) break; digest.update(buffer, 0, count) }
    }
    return digestHex(digest.digest())
}

/** A retry rehashes the checkpoint, then hashes incoming chunks. ETag/size/range are checked before writing. */
internal suspend fun downloadReleaseFiles(source: UpdateSource, files: List<ReleaseFile>, root: Path, report: (UpdateState) -> Unit) = withContext(Dispatchers.IO) {
    validateReleaseFiles(files)
    val staging = guardedUpdateDirectory(root)
    val partials = guardedUpdateDirectory(staging.resolve("parts"))
    val destination = guardedUpdateDirectory(staging.resolve("source"))
    val total = files.sumOf { it.propertySize }
    var complete = 0L
    for (file in files) {
        currentCoroutineContext().ensureActive()
        val target = updatePath(destination, file.path)
        val partial = updatePath(partials, file.sha256)
        if (Files.isRegularFile(target, LinkOption.NOFOLLOW_LINKS) && Files.size(target) == file.propertySize && fileDigest(target) == file.sha256) {
            complete += file.propertySize; report(UpdateState.Downloading(complete, total)); continue
        }
        if (!Files.exists(partial, LinkOption.NOFOLLOW_LINKS)) Files.createFile(partial, updateFileMode)
        require(Files.isRegularFile(partial, LinkOption.NOFOLLOW_LINKS))
        Files.setPosixFilePermissions(partial, PosixFilePermissions.fromString("rw-------"))
        var offset = Files.size(partial).takeIf { it < file.propertySize } ?: 0L
        val digest = MessageDigest.getInstance("SHA-256")
        if (offset > 0) Files.newInputStream(partial, LinkOption.NOFOLLOW_LINKS).use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) { currentCoroutineContext().ensureActive(); val count = input.read(buffer); if (count < 0) break; digest.update(buffer, 0, count) }
        }
        var received = offset
        var began = false
        fun verify(valid: Boolean) { if (!valid) throw UpdateFailure(DOWNLOAD_MISMATCH) }
        try {
            FileChannel.open(partial, StandardOpenOption.WRITE, LinkOption.NOFOLLOW_LINKS).use { out ->
                out.position(offset)
                source.download(file, offset, { head ->
                    verify(!began && head.etag == "\"${file.sha256}\"")
                    if (head.status == 200) {
                        verify(head.length == file.propertySize)
                        out.truncate(0); out.position(0); digest.reset(); received = 0; offset = 0
                    } else verify(head.status == 206 && offset > 0 && head.length == file.propertySize - offset &&
                        head.contentRange == "bytes $offset-${file.propertySize - 1}/${file.propertySize}")
                    began = true
                }, { bytes ->
                    currentCoroutineContext().ensureActive()
                    verify(began && bytes.size <= file.propertySize - received)
                    val buffer = ByteBuffer.wrap(bytes); while (buffer.hasRemaining()) out.write(buffer)
                    digest.update(bytes); received += bytes.size
                    report(UpdateState.Downloading(complete + received, total))
                })
                out.force(true)
            }
            // A truncated connection is resumable; a completed wrong digest is not.
            if (received != file.propertySize) throw UpdateFailure("The download was interrupted. Retry to resume.")
            report(UpdateState.Verifying)
            verify(began && digestHex(digest.digest()) == file.sha256)
            Files.move(partial, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: UpdateFailure) { if (e.reason == DOWNLOAD_MISMATCH) { Files.deleteIfExists(partial); Files.deleteIfExists(target) }; throw e }
        complete += file.propertySize
    }
    removeUpdateTree(partials)
    destination
}
