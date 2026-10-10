package app.porchlight.shared.screens

import kotlinx.datetime.LocalDate
import kotlinx.datetime.LocalDateTime
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

internal actual fun localizedDate(date: LocalDate): String = java.time.LocalDate.of(date.year, date.month.ordinal + 1, date.day)
    .format(DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(Locale.getDefault()))
internal actual fun localizedDateTime(date: LocalDateTime): String = java.time.LocalDateTime.of(date.year, date.month.ordinal + 1, date.day, date.hour, date.minute, date.second)
    .format(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT).withLocale(Locale.getDefault()))

@OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)
internal actual fun addressClipEntry(text: String): androidx.compose.ui.platform.ClipEntry = androidx.compose.ui.platform.ClipEntry(java.awt.datatransfer.StringSelection(text))
