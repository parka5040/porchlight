package app.porchlight.shared.live

import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.encodeToString
import java.nio.file.*

/** Per-code checkpoints: unchanged tickets are not rewritten; nextCode is durable before any new row. */
class DesktopAlarmStore(private val directory: Path) {
    private val rows = directory.resolve("alarm-tickets")
    private val counter = rows.resolve("next-code")
    private val legacy = directory.resolve("alarm-ledger.json")
    private val codes = mutableMapOf<String, Int>()
    private var savedNextCode = 0
    var recoveredDamage = false; private set
    private fun remember(ledger: AlarmLedger): AlarmLedger {
        codes.clear(); ledger.all().forEach { codes[it.key] = it.code }; savedNextCode = ledger.nextCode
        return ledger
    }
    fun load(): AlarmLedger = try { loadIntact() } catch (_: Exception) {
        // Recover allocation high-water before quarantine, including the previous quarantines.
        var maximum = 0
        Files.list(directory).use { dirs -> dirs.filter { it.fileName.toString().startsWith("alarm-tickets") }.forEach { dir ->
            if (Files.isDirectory(dir)) Files.list(dir).use { files -> files.forEach { file ->
                file.fileName.toString().removeSuffix(".json").toIntOrNull()?.let { maximum = maxOf(maximum, it) }
                if (file.fileName.toString() == "next-code") runCatching { Files.readString(file).trim().toInt() - 1 }.getOrNull()?.let { maximum = maxOf(maximum, it) }
            } }
        } }
        if (Files.exists(legacy)) {
            runCatching { Files.readString(legacy) }.getOrNull()?.let { text ->
                Regex("\"(?:code|nextCode)\"\\s*:\\s*(\\d+)").findAll(text).forEach { match ->
                    match.groupValues[1].toIntOrNull()?.let { maximum = maxOf(maximum, it) }
                }
            }
            Files.move(legacy, legacy.resolveSibling("alarm-ledger.damaged-${java.util.UUID.randomUUID()}.json"))
        }
        check(maximum < Int.MAX_VALUE) { "Alarm request codes exhausted" }
        Files.move(rows, rows.resolveSibling("alarm-tickets.damaged-${java.util.UUID.randomUUID()}"), StandardCopyOption.ATOMIC_MOVE)
        Files.createDirectories(rows)
        val ledger = AlarmLedger.restore(maximum + 1, emptyList())
        atomic(counter, ledger.nextCode.toString()); recoveredDamage = true
        remember(ledger)
    }
    private fun loadIntact(): AlarmLedger {
        Files.createDirectories(rows)
        if (Files.exists(legacy)) {
            val ledger = AlarmLedger.decode(Files.readString(legacy))
            atomic(counter, ledger.nextCode.toString())
            ledger.all().forEach { atomic(rows.resolve("${it.code}.json"), WireJson.encodeToString(it)) }
            Files.delete(legacy)
            return remember(ledger)
        }
        val next = if (Files.exists(counter)) Files.readString(counter).toInt() else 1
        val tickets = Files.list(rows).use { paths -> paths.filter { it.fileName.toString().endsWith(".json") }
            .map { WireJson.decodeFromString<AlarmTicket>(Files.readString(it)) }.toList() }
        return remember(AlarmLedger.restore(next, tickets))
    }
    fun save(ledger: AlarmLedger) {
        val changes = ledger.changes()
        if (changes.isEmpty()) return
        if (savedNextCode != ledger.nextCode) { atomic(counter, ledger.nextCode.toString()); savedNextCode = ledger.nextCode }
        changes.forEach { (key, row) ->
            if (row == null) codes[key]?.let { Files.deleteIfExists(rows.resolve("$it.json")); codes.remove(key) }
            else { atomic(rows.resolve("${row.code}.json"), WireJson.encodeToString(row)); codes[key] = row.code }
        }
        ledger.checkpoint()
    }
    private fun atomic(path: Path, text: String) {
        val next = path.resolveSibling(path.fileName.toString() + ".new")
        java.nio.channels.FileChannel.open(next, StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING, StandardOpenOption.WRITE).use { channel ->
            val buffer = java.nio.ByteBuffer.wrap(text.toByteArray(Charsets.UTF_8))
            while (buffer.hasRemaining()) channel.write(buffer)
            channel.force(true)
        }
        Files.move(next, path, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
        java.nio.channels.FileChannel.open(path.parent, StandardOpenOption.READ).use { it.force(true) }
    }
}
