package app.porchlight.shared.files

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.lang.foreign.Arena
import java.lang.foreign.FunctionDescriptor
import java.lang.foreign.Linker
import java.lang.foreign.ValueLayout.*

internal class UnsupportedCalendarArchitectureException : IOException("Calendar import supports only x86_64 and aarch64 Linux")

/** Linux desktop only; uses the bundled JDK's FFM API, not reflection into JDK file-descriptor internals. */
internal object LinuxCalendarInput {
    // O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC from the Linux amd64/arm64 UAPI.
    fun openFlags(arch: String): Int = when (arch) {
        "x86_64", "amd64" -> 0x0 or 0x800 or 0x20000 or 0x80000
        "aarch64", "arm64" -> 0x0 or 0x800 or 0x8000 or 0x80000
        else -> throw UnsupportedCalendarArchitectureException()
    }

    fun read(file: File, arch: String = System.getProperty("os.arch")): ByteArray {
        check(System.getProperty("os.name") == "Linux") { "Calendar import requires Linux" }
        // Refuse unsupported ABIs before even initializing the native bindings.
        val flags = openFlags(arch)
        return Native.read(file, flags)
    }

    private object Native {
        private val linker = Linker.nativeLinker()
        private fun function(name: String, result: java.lang.foreign.MemoryLayout, vararg args: java.lang.foreign.MemoryLayout) =
            linker.downcallHandle(linker.defaultLookup().find(name).orElseThrow(), FunctionDescriptor.of(result, *args))
        private val open = function("open", JAVA_INT, ADDRESS, JAVA_INT)
        private val statx = function("statx", JAVA_INT, JAVA_INT, ADDRESS, JAVA_INT, JAVA_INT, ADDRESS)
        private val read = function("read", JAVA_LONG, JAVA_INT, ADDRESS, JAVA_LONG)
        private val close = function("close", JAVA_INT, JAVA_INT)

        fun read(file: File, flags: Int): ByteArray = Arena.ofConfined().use { arena ->
            val fd = open.invokeWithArguments(arena.allocateFrom(file.path), flags) as Int
            if (fd < 0) throw IOException("Unable to open calendar; choose a regular file, not a link")
            try {
                // statx has a fixed-width ABI across Linux architectures. AT_EMPTY_PATH inspects fd, not file.path.
                val stat = arena.allocate(256, 8)
                val status = statx.invokeWithArguments(fd, arena.allocateFrom(""), 0x1000, 1, stat) as Int
                val mode = stat.get(JAVA_SHORT, 28).toInt() and 0xffff
                if (status != 0 || stat.get(JAVA_INT, 0) and 1 == 0 || mode and 0xf000 != 0x8000)
                    throw IOException("Choose a regular calendar file")
                val limit = 5 * 1024 * 1024
                val buffer = arena.allocate(64 * 1024)
                val result = ByteArrayOutputStream()
                while (true) {
                    val count = read.invokeWithArguments(fd, buffer, minOf(buffer.byteSize(), (limit + 1 - result.size()).toLong())) as Long
                    if (count < 0) throw IOException("Unable to read calendar")
                    if (count == 0L) break
                    result.write(buffer.asSlice(0, count).toArray(JAVA_BYTE))
                    require(result.size() <= limit) { "Calendar exceeds 5 MiB" }
                }
                result.toByteArray()
            } finally { close.invokeWithArguments(fd) }
        }
    }
}
