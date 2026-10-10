package app.porchlight.shared.transport

import java.net.ConnectException
import java.net.UnknownHostException
import java.nio.channels.UnresolvedAddressException
import java.time.ZoneId
import java.time.ZoneOffset
import java.util.concurrent.CancellationException
import java.util.logging.Logger
import javax.net.ssl.SSLException

fun isDefiniteConnectFailure(error: Throwable): Boolean {
    val causes = generateSequence(error) { it.cause }.toList()
    return causes.any { it is ConnectException || it is UnknownHostException || it is UnresolvedAddressException } &&
        causes.none { it is SSLException || it is CancellationException }
}

/** Only explicit TLS alerts qualify; never infer revocation from arbitrary IO text. */
fun isCertificateRevoked(error: Throwable): Boolean = generateSequence(error) { it.cause }.any {
    it is SSLException && it.message?.contains("certificate_revoked", ignoreCase = true) == true
}

fun classifyTlsFailure(error: Exception): Exception = when {
    isCertificateRevoked(error) -> DeviceCertificateRevoked(generateSequence<Throwable>(error) { it.cause }
        .any { it is javax.net.ssl.SSLHandshakeException })
    generateSequence<Throwable>(error) { it.cause }.any {
        it is javax.net.ssl.SSLHandshakeException || it is javax.net.ssl.SSLPeerUnverifiedException
    } -> RequestNotSent()
    else -> error // read/write/reset errors have an unknown outcome
}

fun ianaZone(id: String): String? {
    val systemV = mapOf(
        "AST4" to "America/Puerto_Rico", "AST4ADT" to "America/Halifax",
        "EST5" to "America/Panama", "EST5EDT" to "America/New_York",
        "CST6" to "America/Regina", "CST6CDT" to "America/Chicago",
        "MST7" to "America/Phoenix", "MST7MDT" to "America/Denver",
        "PST8" to "Pacific/Pitcairn", "PST8PDT" to "America/Los_Angeles",
        "YST9" to "Pacific/Gambier", "YST9YDT" to "America/Anchorage",
        "HST10" to "Pacific/Honolulu")
    if (id.startsWith("SystemV/")) return systemV[id.substringAfter('/')]
    // Preserve valid tzdb names, including slashless aliases (CET, UTC).
    if (id in availableIanaZones) return id
    if (id in setOf("UT", "Z")) return "Etc/UTC"
    val zone = runCatching { ZoneId.of(id) }.getOrNull() ?: return null
    val normalized = zone.normalized()
    if (normalized is ZoneOffset) {
        val seconds = normalized.totalSeconds
        if (seconds % 3600 != 0) return null // No matching fixed-offset IANA zone: omit, don't invent one.
        if (seconds == 0) return "Etc/UTC"
        val hours = seconds / 3600
        val candidate = "Etc/GMT${if (hours > 0) "-" else "+"}${kotlin.math.abs(hours)}"
        return candidate.takeIf { it in availableIanaZones }
    }
    return id.takeIf { it in availableIanaZones }
}

private val availableIanaZones = ZoneId.getAvailableZoneIds()

/** Checks the cheap system id each request, maps/logs only on a zone change. */
internal class DeviceZoneCache(
    private val deviceId: () -> String,
    private val map: (String) -> String? = ::ianaZone,
    private val unsupported: () -> Unit = {},
) {
    private var lastId: String? = null
    private var mapped: String? = null
    @Synchronized fun current(): String? {
        val id = deviceId()
        if (id != lastId) {
            mapped = map(id)
            lastId = id
            if (mapped == null) unsupported()
        }
        return mapped
    }
}
private val processZone = DeviceZoneCache({ ZoneId.systemDefault().id }, unsupported = {
    Logger.getLogger("PorchlightTransport").warning("System time zone has no supported IANA mapping; omitting Porchlight-TZ")
})
fun systemIanaZone(): String? = processZone.current()
