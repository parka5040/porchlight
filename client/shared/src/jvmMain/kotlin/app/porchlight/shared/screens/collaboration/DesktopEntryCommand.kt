package app.porchlight.shared.screens.collaboration

import java.nio.file.Files
import java.nio.file.Path

internal fun desktopAttachmentHandler(desktop: String, dataHome: String, dataDirs: String): Path? {
    require(desktop.matches(Regex("[a-zA-Z0-9_.-]+\\.desktop")))
    for (root in (listOf(dataHome) + dataDirs.split(':')).filter { it.isNotBlank() && Path.of(it).isAbsolute }) {
        val applications = Path.of(root, "applications")
        if (!Files.isDirectory(applications)) continue
        val entry = Files.walk(applications).use { paths -> paths.filter { path ->
            Files.isRegularFile(path) && applications.relativize(path).joinToString("-") == desktop
        }.sorted().findFirst().orElse(null) } ?: continue
        val fields = desktopEntryFields(readDesktopEntry(entry))
        // The first root owns the ID, even when it tombstones a lower-priority handler.
        return if (fields["Hidden"] == "true") null else entry
    }
    return null
}

internal fun readDesktopEntry(entry: Path): String = Files.newInputStream(entry).use { input ->
    val bytes = input.readNBytes(65_537)
    require(bytes.size <= 65_536) { "Attachment MIME handler is too large" }
    bytes.toString(Charsets.UTF_8)
}

private fun desktopEntryFields(text: String): Map<String, String> {
    val fields = mutableMapOf<String, String>()
    var main = false
    text.lineSequence().forEach { raw ->
        val line = raw.trim()
        when {
            line.startsWith('[') -> main = line == "[Desktop Entry]"
            main && line.isNotEmpty() && !line.startsWith('#') -> {
                val key = line.substringBefore('=').trim()
                require('=' in line && fields.put(key, line.substringAfter('=').trim()) == null) { "Invalid attachment MIME handler" }
            }
        }
    }
    return fields
}

private fun desktopEntryString(raw: String): String = buildString {
    var i = 0
    while (i < raw.length) {
        val c = raw[i++]
        if (c != '\\') append(c) else {
            require(i < raw.length) { "Incomplete desktop entry escape" }
            append(when (raw[i++]) {
                's' -> ' '; 'n' -> '\n'; 't' -> '\t'; 'r' -> '\r'; '\\' -> '\\'
                else -> error("Unsupported desktop entry escape")
            })
        }
    }
}

/** Desktop Entry string escaping precedes Exec quoting. No command is ever evaluated by a shell. */
internal fun desktopEntryCommand(text: String, alias: String, desktopFile: String? = null): List<String> {
    val fields = desktopEntryFields(text)
    require(fields["Type"] == "Application" && fields["Terminal"] in listOf(null, "false") && fields["Hidden"] in listOf(null, "false")) { "Unsupported attachment MIME handler" }
    val exec = desktopEntryString(requireNotNull(fields["Exec"]) { "Missing attachment viewer command" })
    val args = mutableListOf<Pair<String, Boolean>>()
    var i = 0
    while (i < exec.length) {
        while (i < exec.length && exec[i].isWhitespace()) i++
        if (i == exec.length) break
        val quoted = exec[i] == '"'
        if (quoted) i++
        val arg = buildString {
            var closed = !quoted
            while (i < exec.length) {
                val c = exec[i++]
                if (quoted && c == '"') { closed = true; break }
                if (!quoted && c.isWhitespace()) break
                if (quoted && c == '\\') {
                    require(i < exec.length && exec[i] in "\"`$\\") { "Unsupported Exec escape" }
                    append(exec[i++])
                } else {
                    require(quoted || c !in "\"'\\><~|&;$*?#()`") { "Unquoted Exec reserved character" }
                    append(c)
                }
            }
            require(closed) { "Unterminated Exec quote" }
        }
        require(!quoted || i == exec.length || exec[i].isWhitespace()) { "Exec arguments must be separated" }
        args += arg to quoted
    }
    require(args.isNotEmpty() && args.first().first.isNotBlank() && '%' !in args.first().first) { "Missing attachment viewer executable" }
    var fileCode = false
    val command = args.flatMap { (arg, quoted) ->
        when {
            arg in listOf("%f", "%F", "%u", "%U") -> {
                require(!quoted && !fileCode) { "Unsupported attachment file code" }
                fileCode = true
                listOf(alias)
            }
            arg == "%i" -> {
                require(!quoted)
                fields["Icon"]?.takeIf { it.isNotEmpty() }?.let { listOf("--icon", desktopEntryString(it)) } ?: emptyList()
            }
            arg == "%c" -> { require(!quoted); listOf(desktopEntryString(requireNotNull(fields["Name"]) { "Missing attachment viewer name" })) }
            arg == "%k" -> { require(!quoted); listOf(requireNotNull(desktopFile) { "Missing desktop entry path" }) }
            else -> listOf(buildString {
                var index = 0
                while (index < arg.length) {
                    val c = arg[index++]
                    if (c != '%') append(c) else {
                        require(!quoted && index < arg.length && arg[index++] == '%') { "Unsupported attachment viewer field code" }
                        append('%')
                    }
                }
            })
        }
    }
    return if (fileCode) command else command + alias
}
