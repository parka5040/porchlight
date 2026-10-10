package app.porchlight.shared.screens.calendar

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.api.models.AvailabilityResponse
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.ui.memberColor
import kotlinx.datetime.*

/** Geometry is supplied by CalendarGrid; this layer only scales minutes, columns and lanes into dp. */
@Composable fun DesktopCalendarGrid(grid: CalendarGrid, today: LocalDate, zone: TimeZone,
    members: List<MemberRead>, openItem: (AppRoute.Item) -> Unit, availability: List<AvailabilityResponse> = emptyList(), visible: Set<String> = emptySet()) {
    val scale = LocalDensity.current.fontScale.coerceAtLeast(1f)
    val hourHeight = grid.scale.hourHeightDp.dp
    val maxColumns = grid.days.maxOfOrNull { day -> day.timed.maxOfOrNull { it.columns } ?: 1 } ?: 1
    val dayWidth = (if (grid.days.size == 1) 560.dp else 300.dp).coerceAtLeast(160.dp * maxColumns) * scale
    val axisWidth = 100.dp * scale
    val columnWidth = axisWidth + dayWidth
    val laneHeight = 220.dp * scale
    val palette = LocalPorchlightPalette.current
    Text("Hour grid · ${zone.id} · each day has its own local-time axis (DST labels include UTC offset)", style = MaterialTheme.typography.bodySmall)
    Column(Modifier.horizontalScroll(rememberScrollState()).testTag("calendar-hour-grid")) {
        Row {
            grid.days.forEach { day ->
                Column(Modifier.width(columnWidth).background(if (day.date == today) palette.surfaceVariant else palette.surface).padding(8.dp)) {
                    Text(day.date.dayOfWeek.name.take(3), color = if (day.date == today) palette.primary else palette.onSurface)
                    Text(day.date.toString(), style = MaterialTheme.typography.titleMedium)
                    if (day.hours != 24L) Text("${day.hours}-hour day · daylight saving transition")
                }
            }
        }
        Text("All day", Modifier.padding(8.dp))
        Row {
            val lanes = (grid.allDay.maxOfOrNull { it.lane }?.plus(1) ?: 1)
            Box(Modifier.width(columnWidth * grid.days.size).height(laneHeight * lanes)) {
                if (grid.allDay.isEmpty()) Text("No all-day items", Modifier.padding(8.dp))
                grid.allDay.forEach { span ->
                    GridEntry(span.entry, zone, members, openItem, true, true,
                        Modifier.offset(x = columnWidth * span.firstDay + axisWidth, y = laneHeight * span.lane)
                            .width(columnWidth * span.dayCount - axisWidth).height(laneHeight).padding(4.dp))
                }
            }
        }
        HorizontalDivider()
        Row {
            grid.days.forEach { day ->
                // Never borrow Sunday's axis: other days in a DST week have different elapsed-to-wall mappings.
                Box(Modifier.width(columnWidth).height(hourHeight * day.hours.toInt()).testTag("calendar-day-${day.date}")) {
                    Canvas(Modifier.matchParentSize()) {
                        day.ticks.forEach { tick ->
                            val y = hourHeight.toPx() * (tick.minute / 60).toFloat()
                            drawLine(palette.outline, Offset(axisWidth.toPx(), y), Offset(size.width, y))
                        }
                        drawLine(palette.outline, Offset.Zero, Offset(0f, size.height))
                    }
                    availabilityBands(day.date, zone, availability, members, visible).forEach { band ->
                        val color = members.find { it.id == band.memberId }?.let { memberColor(it.color) } ?: palette.primary
                        Box(Modifier.offset(x = axisWidth, y = hourHeight * (band.startMinute / 60).toFloat())
                            .width(dayWidth).height(hourHeight * (band.durationMinutes / 60).toFloat())
                            .background(color.copy(alpha = 0.1f)).semantics { contentDescription = band.label })
                    }
                    day.ticks.forEach { tick ->
                        Text(tick.label, Modifier.offset(y = hourHeight * (tick.minute / 60).toFloat()).width(axisWidth).padding(4.dp), style = MaterialTheme.typography.labelSmall)
                    }
                    day.timed.forEach { entry ->
                        val width = dayWidth / entry.columns
                        val height = hourHeight * (entry.displayDurationMinutes / 60).toFloat()
                        GridEntry(entry, zone, members, openItem, false, entry.durationMinutes >= 60,
                            Modifier.offset(x = axisWidth + width * entry.column, y = hourHeight * (entry.startMinute / 60).toFloat())
                                .width(width).height(height).padding(2.dp))
                    }
                }
            }
        }
    }
}

@Composable private fun GridEntry(entry: CalendarEntry, zone: TimeZone, members: List<MemberRead>, openItem: (AppRoute.Item) -> Unit,
    allDay: Boolean, roomy: Boolean, modifier: Modifier) {
    val palette = LocalPorchlightPalette.current
    val member = members.find { it.id == entry.item.agenda?.memberId }
    val color = member?.let { memberColor(it.color) } ?: palette.outline
    val conflicts = entry.item.conflicts.orEmpty().any { it.memberId == entry.item.agenda?.memberId }
    val labels = calendarEntryLabels(entry, zone, allDay)
    val shape = RoundedCornerShape(8.dp)
    val canOpen = "view" in entry.item.capabilities
    val action = if (canOpen) Modifier.clickable(role = Role.Button, onClickLabel = "Open ${entry.item.title}") { openItem(calendarItemRoute(entry)) } else Modifier
    Surface(modifier.then(action).semantics(mergeDescendants = true) {
        contentDescription = if (canOpen) "Open ${entry.item.title}" else entry.item.title
        stateDescription = (listOfNotNull(member?.displayName) + labels).joinToString(" · ")
    }, shape = shape, color = color.copy(alpha = 0.16f), contentColor = palette.onSurface,
        border = BorderStroke(if (conflicts) 2.dp else 1.dp, if (conflicts) palette.errorText else color)) {
        Column(Modifier.padding(8.dp)) {
            Text((if (!roomy && conflicts) "Conflict · " else "") + entry.item.title, style = MaterialTheme.typography.titleSmall,
                maxLines = if (roomy) 2 else 1, overflow = TextOverflow.Ellipsis)
            if (roomy) {
                member?.let { Text(it.initials, style = MaterialTheme.typography.labelSmall) }
                // Short blocks omit subtitles; long ones show a compact warning. Complete labels remain in semantics.
                labels.take(3).forEach { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                if (labels.size > 3) Text("More warnings · ${labels.size - 3}", style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}
