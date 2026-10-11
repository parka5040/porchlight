package app.porchlight.shared.files

import java.io.File
import java.io.Reader
import java.nio.charset.CodingErrorAction

/** Syntax-only streaming validation: no table, row or ICS string is materialized in memory. */
internal fun validateMemberExport(file: File, checkGate: () -> Unit) {
    val decoder = Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
    java.io.InputStreamReader(file.inputStream(), decoder).use { MemberJsonValidator(it, checkGate).validate() }
}

private class MemberJsonValidator(private val input: Reader, private val checkGate: () -> Unit) {
    private val buffer = CharArray(8192)
    private var size = 0
    private var position = 0
    private fun peek(): Int {
        if (position == size) {
            checkGate()
            size = input.read(buffer); position = 0
            if (size < 0) { size = 0; return -1 }
        }
        return buffer[position].code
    }
    private fun next(): Int = peek().also { if (it >= 0) position++ }
    private fun invalid(): Nothing = throw IllegalArgumentException("Incomplete or invalid member export")
    private fun take(c: Char) { if (next() != c.code) invalid() }
    private fun whitespace() { while (peek() in listOf(32, 9, 10, 13)) next() }
    private fun string(capture: Boolean = false): String? {
        take('"')
        val text = if (capture) StringBuilder() else null
        var overflow = false
        while (true) {
            val c = next()
            if (c == '"'.code) return if (overflow) null else text?.toString()
            if (c < 32) invalid()
            val decoded = if (c != '\\'.code) c.toChar() else when (val escape = next().toChar()) {
                '"', '\\', '/' -> escape
                'b' -> '\b'; 'f' -> '\u000c'; 'n' -> '\n'; 'r' -> '\r'; 't' -> '\t'
                'u' -> {
                    var code = 0
                    repeat(4) {
                        val digit = when (val hex = next()) {
                            in '0'.code..'9'.code -> hex - '0'.code
                            in 'a'.code..'f'.code -> hex - 'a'.code + 10
                            in 'A'.code..'F'.code -> hex - 'A'.code + 10
                            else -> invalid()
                        }
                        code = code * 16 + digit
                    }
                    code.toChar()
                }
                else -> invalid()
            }
            if (text != null && !overflow) { if (text.length < 64) text.append(decoded) else overflow = true }
        }
    }
    private fun number(): Boolean {
        var versionOne = true
        if (peek() == '-'.code) { next(); versionOne = false }
        val first = next()
        if (first !in '0'.code..'9'.code) invalid()
        if (first != '1'.code) versionOne = false
        if (first != '0'.code) while (peek() in '0'.code..'9'.code) { next(); versionOne = false }
        if (peek() == '.'.code) {
            next(); versionOne = false
            if (next() !in '0'.code..'9'.code) invalid()
            while (peek() in '0'.code..'9'.code) next()
        }
        if (peek() == 'e'.code || peek() == 'E'.code) {
            next(); versionOne = false
            if (peek() == '+'.code || peek() == '-'.code) next()
            if (next() !in '0'.code..'9'.code) invalid()
            while (peek() in '0'.code..'9'.code) next()
        }
        return versionOne
    }
    private fun sequence(open: Char, close: Char, entry: () -> Unit) {
        take(open); whitespace()
        if (peek() == close.code) { next(); return }
        while (true) {
            entry(); whitespace()
            when (next()) { close.code -> return; ','.code -> whitespace(); else -> invalid() }
        }
    }
    private fun value(depth: Int) {
        if (depth > 128) invalid()
        whitespace()
        when (peek()) {
            '{'.code -> sequence('{', '}') { string(); whitespace(); take(':'); value(depth + 1) }
            '['.code -> sequence('[', ']') { value(depth + 1) }
            '"'.code -> string()
            't'.code -> "true".forEach(::take)
            'f'.code -> "false".forEach(::take)
            'n'.code -> "null".forEach(::take)
            else -> number()
        }
    }
    fun validate() {
        var version = false; var tables = false; var calendar = false
        whitespace()
        sequence('{', '}') {
            val name = string(capture = true); whitespace(); take(':'); whitespace()
            when (name) {
                "format_version" -> { if (version || !number()) invalid(); version = true }
                "tables" -> { if (tables || peek() != '{'.code) invalid(); tables = true; value(1) }
                "appointments_ics" -> { if (calendar) invalid(); string(); calendar = true }
                else -> value(1)
            }
        }
        whitespace()
        if (!version || !tables || !calendar || peek() != -1) invalid()
    }
}
