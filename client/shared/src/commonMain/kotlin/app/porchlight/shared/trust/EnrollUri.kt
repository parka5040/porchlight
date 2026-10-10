package app.porchlight.shared.trust

import io.ktor.http.decodeURLQueryComponent

/** URI parsing mirrors core EnrollUri; target validation is deliberately a separate operation. */
class EnrollUri private constructor(val host: String, val token: String, val caHex: String) {
    override fun toString() = "EnrollUri(host=…redacted, token=…redacted, ca=sha256:$caHex)"
    override fun equals(other: Any?) = other is EnrollUri && host == other.host && token == other.token && caHex == other.caHex
    override fun hashCode() = host.hashCode() * 31 + token.hashCode() * 17 + caHex.hashCode()
    companion object {
        fun parse(text: String): EnrollUri {
            fun invalid(): Nothing = throw IllegalArgumentException("Invalid enrollment URI")
            fun decode(value: String): String = try { value.decodeURLQueryComponent(plusIsSpace = true) }
                catch (_: Exception) { invalid() } // URLDecodeException includes the raw token; never preserve it.
            val cleaned = text.trim { it.code <= 32 }.replace("\t", "").replace("\r", "").replace("\n", "")
            val match = Regex("(?i:porchlight)://enroll/?\\?(.*)").matchEntire(cleaned) ?: invalid()
            if ('#' in cleaned) invalid()
            val known = mutableMapOf<String, String>()
            for (pair in match.groupValues[1].split('&')) {
                val key = decode(pair.substringBefore('='))
                if (key !in listOf("host", "token", "ca")) continue
                val value = decode(pair.substringAfter('=', ""))
                if (value.isEmpty() || known.put(key, value) != null) invalid()
            }
            val host = known["host"] ?: invalid()
            val token = known["token"] ?: invalid()
            val ca = known["ca"] ?: invalid()
            if (!Regex("(?i:sha256):[a-fA-F0-9]{64}").matches(ca)) invalid()
            return EnrollUri(host, token, ca.substringAfter(':').lowercase())
        }
    }
}

private fun validIpv6(name: String): Boolean {
    if (':' !in name) return false
    val halves = name.split("::")
    if (halves.size > 2) return false
    val groups = halves.flatMap { if (it.isEmpty()) emptyList() else it.split(':') }
    var count = 0
    for ((i, group) in groups.withIndex()) {
        if ('.' in group) {
            val octets = group.split('.')
            if (i != groups.lastIndex || octets.size != 4 || octets.any { it.isEmpty() || it.any { c -> c !in '0'..'9' } || (it.length > 1 && it[0] == '0') || it.toIntOrNull() !in 0..255 }) return false
            count += 2
        } else {
            if (group.length !in 1..4 || group.any { it !in "0123456789abcdefABCDEF" }) return false
            count++
        }
    }
    return if (halves.size == 2) count < 8 else count == 8
}

class EnrollmentTarget private constructor(val host: String, val port: Int, val baseUrl: String) {
    companion object {
        fun parse(authority: String): EnrollmentTarget {
            require(authority.isNotEmpty() && authority.none { it.isWhitespace() || it in "/@?#\\" }) { "Invalid enrollment target" }
            val bracket = authority.startsWith('[')
            val parts = if (bracket) {
                val end = authority.indexOf(']')
                require(end > 1) { "Invalid enrollment target" }
                val name = authority.substring(1, end)
                require(validIpv6(name)) { "Invalid enrollment target" }
                val suffix = authority.substring(end + 1)
                require(suffix.isEmpty() || suffix.startsWith(':')) { "Invalid enrollment target" }
                name to if (suffix.isEmpty()) null else suffix.substring(1)
            } else {
                val split = authority.split(':')
                require(split.size <= 2 && Regex("[a-zA-Z0-9_.-]+").matches(split[0])) { "Invalid enrollment target" }
                split[0] to split.getOrNull(1)
            }
            val port = parts.second?.let {
                require(it.isNotEmpty() && it.all { c -> c in '0'..'9' }) { "Invalid enrollment target" }
                it.toIntOrNull()?.takeIf { p -> p in 1..65535 } ?: error("Invalid enrollment target")
            } ?: 8444
            return EnrollmentTarget(parts.first, port, "https://${if (bracket) "[${parts.first}]" else parts.first}:$port")
        }
    }
}
