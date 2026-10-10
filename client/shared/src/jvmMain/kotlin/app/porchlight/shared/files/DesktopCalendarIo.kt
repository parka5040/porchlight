package app.porchlight.shared.files

import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.nio.channels.Channels
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption
import java.nio.file.attribute.PosixFilePermission
import java.nio.file.attribute.PosixFilePermissions

const val EXPORT_NOT_PRIVATE_WARNING =
    "Saved \u2014 this location can't keep the file private; others with access to this drive may read it."

/**
 * Atomic 0600 creation, like DesktopKeyStore. No overwrite. Where the filesystem cannot take POSIX creation
 * attributes at all the file is created without them; [desktopExportWarning] then reports it as not private.
 */
internal fun openDesktopExport(file: File): OutputStream {
    val options = setOf(StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE)
    val channel = try {
        Files.newByteChannel(file.toPath(), options,
            PosixFilePermissions.asFileAttribute(OWNER_ONLY_FILE))
    } catch (e: UnsupportedOperationException) { Files.newByteChannel(file.toPath(), options) }
    return Channels.newOutputStream(channel)
}

/**
 * Effective permissions of a written export: filesystems such as vfat/exFAT ignore the requested 0600 and apply the
 * mount's mode. Returns a user-facing warning when any group/other bit is set or privacy cannot be confirmed.
 * [readPermissions] is the seam; null means POSIX attributes are unsupported.
 */
internal fun desktopExportWarning(
    file: File,
    readPermissions: (Path) -> Set<PosixFilePermission>? = {
        try { Files.getPosixFilePermissions(it) } catch (e: UnsupportedOperationException) { null }
    },
): String? {
    val permissions = try { readPermissions(file.toPath()) } catch (e: Exception) { null }
    val private = permissions != null && permissions.none { it.name.startsWith("GROUP") || it.name.startsWith("OTHERS") }
    return if (private) null else EXPORT_NOT_PRIVATE_WARNING
}

/** Never trust stat size: a file can grow, be replaced, or name a size-zero stream. */
internal fun readDesktopCalendar(input: InputStream): ByteArray {
    val limit = 5 * 1024 * 1024
    val bytes = input.readNBytes(limit + 1)
    require(bytes.size <= limit) { "Calendar exceeds 5 MiB" }
    return bytes
}
