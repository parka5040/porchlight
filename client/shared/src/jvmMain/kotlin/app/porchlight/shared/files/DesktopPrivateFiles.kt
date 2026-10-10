package app.porchlight.shared.files

import java.io.File
import java.io.OutputStream
import java.nio.channels.FileChannel
import java.nio.file.AccessDeniedException
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.PosixFileAttributes
import java.nio.file.attribute.PosixFilePermissions

private val OWNER_ONLY_DIRECTORY = PosixFilePermissions.fromString("rwx------")
internal val OWNER_ONLY_FILE = PosixFilePermissions.fromString("rw-------")

/**
 * Porchlight's own private directory (the desktop attachment cache): missing levels are created 0700 at creation and
 * an existing directory is narrowed to 0700. Unlike an export destination the user picks ([desktopExportWarning]),
 * this location must be private, so there is no warn-and-continue path: a symlinked, non-directory, foreign-owned or
 * unwritable location, or a filesystem without POSIX permissions, fails closed with [PrivateDirectoryUnavailable]
 * (a persistent condition the user must fix; retrying does not help) and nothing is written.
 */
internal fun ensurePrivateDesktopDirectory(dir: Path): Path {
    fun unavailable(reason: String): Nothing = throw PrivateDirectoryUnavailable(dir.toString(), reason)
    try {
        Files.createDirectories(dir, PosixFilePermissions.asFileAttribute(OWNER_ONLY_DIRECTORY))
    } catch (_: FileAlreadyExistsException) { unavailable(if (Files.isSymbolicLink(dir)) "it is a symbolic link" else "it exists but is not a folder")
    } catch (_: UnsupportedOperationException) { unavailable("its filesystem cannot keep files private (no owner-only permissions)")
    } catch (_: AccessDeniedException) { unavailable("you are not allowed to create it") }
    val attributes = try { Files.readAttributes(dir, PosixFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS) }
        catch (_: UnsupportedOperationException) { unavailable("its filesystem cannot keep files private (no owner-only permissions)") }
    if (attributes.isSymbolicLink) unavailable("it is a symbolic link")
    if (!attributes.isDirectory) unavailable("it exists but is not a folder")
    // chmod succeeds only for the owner, so a foreign-owned directory that is not already 0700 is refused here; one
    // that is (or that we cannot write) is refused when the staging file is created.
    try { if (attributes.permissions() != OWNER_ONLY_DIRECTORY) Files.setPosixFilePermissions(dir, OWNER_ONLY_DIRECTORY) }
    catch (_: java.nio.file.FileSystemException) { unavailable("it belongs to another user or its permissions cannot be changed") }
    if (Files.getPosixFilePermissions(dir, LinkOption.NOFOLLOW_LINKS) != OWNER_ONLY_DIRECTORY) unavailable("its permissions cannot be made owner-only")
    return dir
}

/**
 * Writes [name] in [dir] owner-only and atomically, like [openDesktopExport] (c5-rr1 OOS): the bytes go to a staging
 * file that is created 0600 (never with the ambient umask and narrowed later) inside the private [dir], then the
 * staging file is renamed over [name]. A symlink or an older file at [name] is replaced, never followed or reused.
 * Staging names end in `-part`: the attachment store's cleanup and [deletePrivateStagingLeftovers] remove them after a crash.
 */
internal fun writePrivateDesktopFile(dir: File, name: String, write: (OutputStream) -> Unit): File =
    writePrivateDesktopFile(dir.toPath(), name, durable = false, write).toFile()

/** As above; [durable] (deskkey: the desktop key secret) also fsyncs the file before the rename and the folder after. */
internal fun writePrivateDesktopFile(dir: Path, name: String, durable: Boolean, write: (OutputStream) -> Unit): Path {
    requirePrivateName(name)
    val root = ensurePrivateDesktopDirectory(dir)
    val staging = try { Files.createTempFile(root, ".staging-", "-part", PosixFilePermissions.asFileAttribute(OWNER_ONLY_FILE)) }
        catch (_: AccessDeniedException) { throw PrivateDirectoryUnavailable(root.toString(), "it belongs to another user or is not writable") }
    try {
        Files.newOutputStream(staging, StandardOpenOption.WRITE, StandardOpenOption.TRUNCATE_EXISTING, LinkOption.NOFOLLOW_LINKS).use(write)
        if (durable) FileChannel.open(staging, StandardOpenOption.WRITE).use { it.force(true) }
        val target = root.resolve(name)
        Files.move(staging, target, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        if (durable) FileChannel.open(root, StandardOpenOption.READ).use { it.force(true) }
        return target
    } finally { Files.deleteIfExists(staging) }
}

private fun requirePrivateName(name: String) = require(name.isNotEmpty() && '/' !in name && name != "." && name != "..")

/**
 * Reads [name] from Porchlight's private [dir] (deskkey), or null when the folder or the file does not exist yet; a
 * missing folder is not created. An existing folder gets [ensurePrivateDesktopDirectory]'s checks. The file must be a
 * regular file owned by the folder's owner: a symlink is refused, never followed, and a file that cannot be read
 * (another user's folder) fails closed with [PrivateDirectoryUnavailable] instead of looking absent. A file whose
 * mode was widened is narrowed back to 0600, as the folder is to 0700.
 */
internal fun readPrivateDesktopFile(dir: Path, name: String): ByteArray? {
    requirePrivateName(name)
    if (!Files.exists(dir, LinkOption.NOFOLLOW_LINKS)) return null
    val root = ensurePrivateDesktopDirectory(dir)
    fun unavailable(reason: String): Nothing = throw PrivateDirectoryUnavailable(root.toString(), reason)
    val target = root.resolve(name)
    val attributes = try { Files.readAttributes(target, PosixFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS) }
        catch (_: NoSuchFileException) { return null }
        catch (_: AccessDeniedException) { unavailable("it belongs to another user or is not readable") }
    if (attributes.isSymbolicLink) unavailable("$name in it is a symbolic link")
    if (!attributes.isRegularFile) unavailable("$name in it is not a regular file")
    if (attributes.owner() != Files.getOwner(root, LinkOption.NOFOLLOW_LINKS)) unavailable("$name in it belongs to another user")
    try { if (attributes.permissions() != OWNER_ONLY_FILE) Files.setPosixFilePermissions(target, OWNER_ONLY_FILE) }
    catch (_: java.nio.file.FileSystemException) { unavailable("the permissions of $name in it cannot be made owner-only") }
    return try { Files.newInputStream(target, LinkOption.NOFOLLOW_LINKS).use { it.readBytes() } }
        catch (_: AccessDeniedException) { unavailable("$name in it is not readable") }
}

/** Deletes [name] from the private [dir] if both exist; an unsafe existing folder fails closed like the readers. */
internal fun deletePrivateDesktopFile(dir: Path, name: String) {
    requirePrivateName(name)
    if (!Files.exists(dir, LinkOption.NOFOLLOW_LINKS)) return
    val root = ensurePrivateDesktopDirectory(dir)
    try { Files.deleteIfExists(root.resolve(name)) }
    catch (_: AccessDeniedException) { throw PrivateDirectoryUnavailable(root.toString(), "it belongs to another user or is not writable") }
}

/**
 * Removes the `.staging-*-part` files a crash left between staging and rename in the private [dir] (deskkey M1).
 * Only those names, only directly inside [dir] (links are removed, never followed); an unsafe folder fails closed.
 */
internal fun deletePrivateStagingLeftovers(dir: Path) {
    if (!Files.exists(dir, LinkOption.NOFOLLOW_LINKS)) return
    val root = ensurePrivateDesktopDirectory(dir)
    Files.newDirectoryStream(root, ".staging-*-part").use { entries ->
        for (entry in entries) if (!Files.isDirectory(entry, LinkOption.NOFOLLOW_LINKS)) Files.deleteIfExists(entry)
    }
}
