package app.porchlight.shared.screens.calendar

import app.porchlight.shared.theme.PorchlightTokens

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.draw.clipToBounds
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
    members: List<MemberRead>, openItem: (AppRoute.Item) -> Unit, availability: List<AvailabilityResponse> = emptyList(),
    visible: Set<String> = emptySet(), viewportHeight: Dp = Dp.Infinity) {
    val scale = LocalDensity.current.fontScale.coerceAtLeast(1f)
    val hourHeight = grid.scale.hourHeightDp.dp
    val maxColumns = grid.days.maxOfOrNull { day -> day.timed.maxOfOrNull { it.columns } ?: 1 } ?: 1
    val dayWidth = (if (grid.days.size == 1) PorchlightTokens.calendarDayWidth.dp else PorchlightTokens.calendarWeekDayWidth.dp).coerceAtLeast(PorchlightTokens.calendarMinColumnWidth.dp * maxColumns) * scale
    val axisWidth = PorchlightTokens.calendarAxisWidth.dp * scale
    val columnWidth = axisWidth + dayWidth
    val laneHeight = PorchlightTokens.calendarLaneHeight.dp * scale
    val palette = LocalPorchlightPalette.current
    Text("Hour grid · ${zone.id} · each day has its own local-time axis (DST labels include UTC offset)", style = MaterialTheme.typography.bodySmall)
    Column(Modifier.horizontalScroll(rememberScrollState()).testTag("calendar-hour-grid")) {
        Row {
            grid.days.forEach { day ->
                Column(Modifier.width(columnWidth).background(if (day.date == today) palette.surfaceVariant else palette.surface).padding(PorchlightTokens.spaceSm.dp)) {
                    Text(day.date.dayOfWeek.name.take(3), color = if (day.date == today) palette.primary else palette.onSurface)
                    Text(day.date.toString(), style = MaterialTheme.typography.titleMedium)
                    if (day.hours != 24L) Text("${day.hours}-hour day · daylight saving transition")
                }
            }
        }
        Text("All day", Modifier.padding(PorchlightTokens.spaceSm.dp))
        Row {
            val lanes = (grid.allDay.maxOfOrNull { it.lane }?.plus(1) ?: 1)
            Box(Modifier.width(columnWidth * grid.days.size).height(laneHeight * lanes)) {
                if (grid.allDay.isEmpty()) Text("No all-day items", Modifier.padding(PorchlightTokens.spaceSm.dp))
                grid.allDay.forEach { span ->
                    GridEntry(span.entry, zone, members, openItem, true, true,
                        Modifier.offset(x = columnWidth * span.firstDay + axisWidth, y = laneHeight * span.lane)
                            .width(columnWidth * span.dayCount - axisWidth).height(laneHeight).padding(PorchlightTokens.spaceXs.dp))
                }
            }
        }
        HorizontalDivider()
        // Scroll the hours, not the navigation/all-day lane. Refreshes keep the user's position; a new period or
        // zone starts at local 08:00 (earliest elapsed 08:00 in DST weeks so no day's morning is hidden).
        key(grid.days.map { it.date }, zone, LocalDensity.current) {
        val workMinute = grid.days.minOfOrNull { day -> day.ticks.firstOrNull { it.label.startsWith("08:00") }?.minute ?: 480.0 } ?: 480.0
        val initial = with(LocalDensity.current) { (hourHeight * (workMinute / 60).toFloat()).roundToPx() }
        val hoursScroll = rememberScrollState(initial)
        val viewport = if (viewportHeight == Dp.Infinity) Modifier else Modifier.heightIn(max = viewportHeight)
            .verticalScroll(hoursScroll).testTag("calendar-hours-scroll")
        Row(viewport) {
            grid.days.forEach { day ->
                // Never borrow Sunday's axis: other days in a DST week have different elapsed-to-wall mappings.
                Box(Modifier.width(columnWidth).height(hourHeight * day.hours.toInt()).clipToBounds().testTag("calendar-day-${day.date}")) {
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
                        Text(tick.label, Modifier.offset(y = hourHeight * (tick.minute / 60).toFloat()).width(axisWidth).padding(PorchlightTokens.spaceXs.dp), style = MaterialTheme.typography.labelSmall)
                    }
                    day.timed.forEach { entry ->
                        val width = dayWidth / entry.columns
                        val height = hourHeight * (entry.displayDurationMinutes / 60).toFloat()
                        GridEntry(entry, zone, members, openItem, false, entry.durationMinutes >= 60,
                            Modifier.offset(x = axisWidth + width * entry.column, y = hourHeight * (entry.startMinute / 60).toFloat())
                                .width(width).height(height).padding(PorchlightTokens.selectionStroke.dp))
                    }
                }
            }
        }
        }
    }
}

@Composable private fun GridEntry(entry: CalendarEntry, zone: TimeZone, members: List<MemberRead>, openItem: (AppRoute.Item) -> Unit,
    allDay: Boolean, roomy: Boolean, modifier: Modifier) {
    val palette = LocalPorchlightPalette.current
    val entryMembers = members.filter { it.id in entry.memberIds }
    val color = entryMembers.firstOrNull()?.let { memberColor(it.color) } ?: palette.outline
    val kinds = entry.item.conflicts.orEmpty().filter { it.memberId in entry.memberIds }.map { it.kind }.toSet()
    val conflicts = kinds.isNotEmpty()
    val compactWarning = listOfNotNull("Over capacity".takeIf { "load" in kinds },
        "Conflict".takeIf { kinds.any { it != "load" } }).joinToString(" · ")
    val initials = entryMembers.joinToString(" / ") { it.initials }
    val labels = calendarEntryLabels(entry, zone, allDay)
    val shape = RoundedCornerShape(PorchlightTokens.spaceSm.dp)
    val canOpen = "view" in entry.item.capabilities
    val action = if (canOpen) Modifier.clickable(role = Role.Button, onClickLabel = "Open ${entry.item.title}") { openItem(calendarItemRoute(entry)) } else Modifier
    Surface(modifier.then(action).semantics(mergeDescendants = true) {
        contentDescription = if (canOpen) "Open ${entry.item.title}" else entry.item.title
        stateDescription = (entryMembers.map { it.displayName } + labels).joinToString(" · ")
    }, shape = shape, color = color.copy(alpha = 0.16f), contentColor = palette.onSurface,
        border = BorderStroke(if (conflicts) PorchlightTokens.selectionStroke.dp else (PorchlightTokens.selectionStroke / 2).dp, if (conflicts) palette.errorText else color)) {
        Column(Modifier.padding(PorchlightTokens.spaceSm.dp)) {
            Text((if (roomy) emptyList() else listOf(initials, compactWarning)).filter { it.isNotEmpty() }
                .plus(entry.item.title).joinToString(" · "), style = MaterialTheme.typography.titleSmall,
                maxLines = if (roomy) 2 else 1, overflow = TextOverflow.Ellipsis)
            if (roomy) {
                if (initials.isNotEmpty()) Text(initials, style = MaterialTheme.typography.labelSmall)
                // Short blocks omit subtitles; long ones show a compact warning. Complete labels remain in semantics.
                labels.take(3).forEach { Text(it, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                if (labels.size > 3) Text("More warnings · ${labels.size - 3}", style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}
