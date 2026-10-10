package app.porchlight.shared.screens.planning

import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale
import kotlin.time.Instant

actual fun formatPlanningTime(value: Instant, zone: String): String {
    val zoneId = runCatching { ZoneId.of(zone) }.getOrDefault(ZoneId.of("UTC"))
    val local = java.time.Instant.ofEpochMilli(value.toEpochMilliseconds()).atZone(zoneId)
    return DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT)
        .withLocale(Locale.getDefault()).format(local) + " · " + local.offset + " (" + zoneId.id + ")"
}
