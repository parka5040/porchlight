package app.porchlight.shared.files

import androidx.compose.runtime.Composable
import app.porchlight.shared.data.ExportSink

data class PickedCalendar(val name: String, val content: String)
interface CalendarFiles {
    fun pick(onResult: (Result<PickedCalendar?>) -> Unit)
    fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit)
    /** As [save]; [onWarning] fires before a successful [onResult] when the file was kept but could not be made private. */
    fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onWarning: (String) -> Unit, onResult: (Result<Boolean>) -> Unit) =
        save(household, write, onResult)
}
@Composable expect fun rememberCalendarFiles(): CalendarFiles
fun validateCalendarFile(name: String, bytes: ByteArray, mime: String? = null): PickedCalendar {
    require(name.endsWith(".ics", ignoreCase = true) || mime == "text/calendar") { "Choose an .ics file" }
    require(bytes.size <= 5 * 1024 * 1024) { "Calendar exceeds 5 MiB" }
    val content = bytes.decodeToString(throwOnInvalidSequence = true).removePrefix("\uFEFF")
    require(content.contains("BEGIN:VCALENDAR")) { "Not an ICS calendar" }
    return PickedCalendar(name, content)
}

/** Stable caller-local file identity; retain existing short scopes and bound long ones to 200 characters. */
fun calendarImportScope(name: String): String {
    val normalized = name.trim().lowercase()
    if (normalized.length <= 195) return "file:$normalized"
    // FNV-1a is a stable suffix, not a security digest. Include the entire name so truncated siblings differ.
    var hash = 0xcbf29ce484222325uL
    normalized.encodeToByteArray().forEach { hash = (hash xor it.toUByte().toULong()) * 0x100000001b3uL }
    val prefix = normalized.take(178).dropLastWhile { it.isHighSurrogate() }
    return "file:$prefix:${hash.toString(16).padStart(16, '0')}"
}

/** Display name the server accepts (1–196 characters, no control characters); keeps the .ics extension when cut. */
fun calendarImportName(name: String): String {
    val clean = name.filterNot { it.isISOControl() }.trim().ifEmpty { "calendar.ics" }
    if (clean.length <= 196) return clean
    val ext = if (clean.endsWith(".ics", ignoreCase = true)) clean.takeLast(4) else ""
    return clean.take(196 - ext.length).dropLastWhile { it.isHighSurrogate() } + ext
}

/** Main-thread picker ownership. Lost/obsolete helpers cannot strand or cancel another request. */
class FileDialogSlot<T> {
    class Request<T>(internal val callback: (T?) -> Unit)
    var current: Request<T>? = null
        private set
    fun begin(callback: (T?) -> Unit): Request<T>? {
        if (current != null) return null
        return Request<T>(callback).also { current = it }
    }
    fun complete(owner: Request<T>, value: T?) {
        if (current !== owner) return
        current = null
        owner.callback(value)
    }
    fun cancel(owner: Request<T>) = complete(owner, null)
    fun discard(owner: Request<T>) { if (current === owner) current = null }
}
