package app.porchlight.shared.screens.calendar

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import app.porchlight.shared.*
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.navigation.*
import app.porchlight.shared.ui.*
import app.porchlight.shared.theme.PorchlightTokens
import app.porchlight.shared.theme.LocalPorchlightPalette
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.selection.selectableGroup
import kotlinx.coroutines.CancellationException
import kotlinx.datetime.*
import kotlin.time.Clock

fun calendarMemberIds(route: AppRoute.Calendar, primary: String?, members: List<MemberRead>): Set<String> =
    setOfNotNull(primary) + route.overlay.orEmpty().split(',').filter { id -> members.any { it.id == id } }

@Composable fun CalendarScreen(services: AppServices?, route: AppRoute.Calendar, navigate: (AppRoute.Calendar) -> Unit, openItem: (AppRoute.Item) -> Unit, openSettings: (SettingsSection) -> Unit = {}) {
    val data = services?.data
    val cache = data?.cache?.view?.collectAsState()?.value ?: CacheView()
    val mobile = services?.platform?.kind == PlatformKind.Android
    val ownId = cache.settings?.memberId
    val primary = route.memberId?.takeIf { id -> cache.members.any { it.id == id } } ?: ownId
    val member = cache.members.find { it.id == primary }
    val zone = remember(member?.tzid) { runCatching { TimeZone.of(member?.tzid ?: "UTC") }.getOrDefault(TimeZone.UTC) }
    val today = (services?.clock?.now() ?: Clock.System.now()).toLocalDateTime(zone).date
    val date = runCatching { LocalDate.parse(route.date ?: today.toString()) }.getOrDefault(today)
    val view = if (mobile && route.view == CalendarView.Month) CalendarView.Agenda else route.view
    val actual = route.copy(view = view, date = date.toString(), memberId = primary)
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    LaunchedEffect(data, actual, zone, retry) {
        if (data == null) return@LaunchedEffect
        loading = true; error = null
        try {
            if (cache.settings == null) { data.household.refreshSettings(); data.household.refreshMembers() }
            val window = calendarWindow(view, date, zone)
            val ids = calendarMemberIds(actual, primary, cache.members)
            if (ids.isEmpty()) data.agenda.refresh(window.from, window.to, null)
            else ids.forEach { data.agenda.refresh(window.from, window.to, it) }
        } catch (e: CancellationException) { throw e }
        catch (_: Exception) { error = "Can't refresh calendar — showing saved data. No changes queued." }
        finally { loading = false }
    }
    CalendarContent(actual, today, zone, primary, cache.agenda, cache.members, mobile, loading, error,
        navigate, openItem, { retry++ }, { navigate(actual.copy(memberId = it)) }, cache.availability) {
            app.porchlight.shared.screens.planning.CalendarPlanningPanel(services, cache, openSettings)
        }
}

/** DESIGN-PENDING: Month, mobile and empty/error layouts. Desktop Day/Week follows DesktopCalendar. */
@OptIn(ExperimentalLayoutApi::class)
@Composable fun CalendarContent(route: AppRoute.Calendar, today: LocalDate, zone: TimeZone, memberId: String?, rows: List<ItemRead>,
    members: List<MemberRead>, mobile: Boolean, loading: Boolean, error: String?, navigate: (AppRoute.Calendar) -> Unit,
    openItem: (AppRoute.Item) -> Unit, refresh: () -> Unit, selectMember: (String) -> Unit = { navigate(route.copy(memberId = it)) },
    availability: List<AvailabilityResponse> = emptyList(),
    gridBuilder: (CalendarWindow, List<ItemRead>, Set<String>, TimeZone, CalendarGridScale) -> CalendarGrid = ::calendarGrid,
    planning: @Composable () -> Unit = {}) {
    val date = runCatching { LocalDate.parse(route.date ?: today.toString()) }.getOrDefault(today)
    val window = remember(route.view, date, zone) { calendarWindow(route.view, date, zone) }
    val visibleIds = remember(route, memberId, members) { calendarMemberIds(route, memberId, members) }
    val visibleRows = remember(window, rows, visibleIds) { calendarRowsInWindow(window, rows, visibleIds) }
    val scale = CalendarGridScale(LocalDensity.current.fontScale)
    val grid = remember(window, visibleRows, visibleIds, zone, scale, gridBuilder) { gridBuilder(window, visibleRows, visibleIds, zone, scale) }
    val days = grid.days
    val dayHeadingYear = window.dates.first().year != window.dates.last().year
    BoxWithConstraints(Modifier.fillMaxSize()) {
    val viewportHeight = maxHeight
    val header: @Composable () -> Unit = {
        val views = listOf(CalendarView.Day, CalendarView.Week, CalendarView.Agenda) + if (mobile) emptyList() else listOf(CalendarView.Month)
        SegmentedChoice(views.map { ScopeOption(it.name, it.name) }, route.view.name, { name ->
            navigate(route.copy(view = CalendarView.valueOf(name), date = date.toString()))
        })
        Column(verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceXs.dp)) {
            Text(calendarPeriodLabel(route.view, date, zone), Modifier.testTag("calendar-period"), style = MaterialTheme.typography.titleLarge)
            Text(calendarZoneLabel(zone), color = LocalPorchlightPalette.current.onSurfaceMuted, style = MaterialTheme.typography.bodySmall)
        }
        val step = if (route.view == CalendarView.Agenda) "7 days" else route.view.name.lowercase()
        ActionRow {
            IconAction("Previous $step", ActionIcon.Previous,
                { navigate(route.copy(date = adjacentDate(route.view, date, -1).toString())) }, Modifier.fillMaxHeight())
            SecondaryAction("Today", {
                if (window != calendarWindow(route.view, today, zone)) navigate(route.copy(date = today.toString()))
            }, Modifier.weight(1f, fill = false).fillMaxHeight())
            IconAction("Next $step", ActionIcon.Next,
                { navigate(route.copy(date = adjacentDate(route.view, date, 1).toString())) }, Modifier.fillMaxHeight())
        }
        SecondaryAction("Refresh calendar", refresh)
        Text("Show calendars", style = MaterialTheme.typography.titleMedium)
        Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        members.forEach { member ->
            MemberChoice(member, member.id == memberId, "Calendar for ${member.displayName}", {
                if (member.id != memberId) selectMember(member.id)
            }) {
                // Desktop retains independent overlays without repeating the member's name on another row.
                if (!mobile) Checkbox(checked = member.id in visibleIds, enabled = member.id != memberId,
                    modifier = Modifier.sizeIn(minWidth = PorchlightTokens.touchTarget.dp, minHeight = PorchlightTokens.touchTarget.dp)
                        .semantics { contentDescription = "Overlay ${member.displayName}" },
                    onCheckedChange = { checked ->
                        val extras = visibleIds - setOfNotNull(memberId)
                        val next = if (checked) extras + member.id else extras - member.id
                        navigate(route.copy(overlay = next.sorted().joinToString(",").ifEmpty { null }))
                    })
            }
        }
        }
        if (loading) Text("Refreshing calendar…")
        if (error != null) {
            OfflineState()
            PorchlightCard(Modifier.fillMaxWidth()) { Text(error, color = LocalPorchlightPalette.current.errorText) }
            SecondaryAction("Try again", refresh, description = "Try again: retry loading")
        }
        if (memberId == null) Text("Sign in and sync to see your calendar.")
        if (mobile || route.view in listOf(CalendarView.Month, CalendarView.Agenda)) DesignPending("Calendar ${route.view.name}")
        else if (error != null || memberId == null) DesignPending("Calendar recovery")
        else if (days.all { it.allDay.isEmpty() && it.timed.isEmpty() }) DesignPending("Calendar empty state")
        if (availability.isNotEmpty() && (mobile || route.view !in listOf(CalendarView.Day, CalendarView.Week))) {
            Text("Availability · shaded windows are available (not appointments)")
            days.forEach { day -> availabilityBands(day.date, zone, availability, members, visibleIds).forEach { Text("${day.date} · ${it.label}") } }
        }
    }
    val agenda = mobile || route.view == CalendarView.Agenda
    if (agenda) {
        LazyColumn(Modifier.fillMaxSize().testTag("calendar-scroll").padding(PorchlightTokens.spaceMd.dp),
            verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
            item("controls", "controls") { Column(verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) { header() } }
            val shown = if (route.view == CalendarView.Agenda) days.filter { it.allDay.isNotEmpty() || it.timed.isNotEmpty() } else days
            if (shown.isEmpty()) item("empty", "empty") { Text("Nothing scheduled") }
            shown.forEach { calendarDayEntries(it, zone, openItem, dayHeadingYear) }
            item("planning", "planning") { planning() }
        }
    } else {
    Column(Modifier.fillMaxSize().testTag("calendar-scroll").verticalScroll(rememberScrollState()).padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
        header()
        if (route.view == CalendarView.Month) {
            Column(Modifier.horizontalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
                days.chunked(7).forEach { week -> Row(horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
                    week.forEach { day -> CalendarDayContent(day, zone, openItem, Modifier.width(PorchlightTokens.calendarMonthDayWidth.dp), dayHeadingYear) }
                } }
            }
        } else if (!mobile && route.view in listOf(CalendarView.Day, CalendarView.Week)) {
            DesktopCalendarGrid(grid, today, zone, members, openItem, availability, visibleIds, viewportHeight)
        } else {
            val shown = if (route.view == CalendarView.Agenda) days.filter { it.allDay.isNotEmpty() || it.timed.isNotEmpty() } else days
            if (shown.isEmpty()) Text("Nothing scheduled")
            shown.forEach { CalendarDayContent(it, zone, openItem, Modifier.fillMaxWidth(), dayHeadingYear) }
        }
        planning()
    }
    }
}
}

private fun LazyListScope.calendarDayEntries(day: CalendarDay, zone: TimeZone, openItem: (AppRoute.Item) -> Unit, includeYear: Boolean) {
    item("day-${day.date}", "day-heading") {
        Text(calendarDayHeading(day.date, includeYear), style = MaterialTheme.typography.titleMedium)
        if (day.hours != 24L) Text("${day.hours}-hour day · daylight saving transition")
    }
    listOf(true to day.allDay, false to day.timed).forEach { (allDay, entries) ->
        val section = "${day.date}-$allDay"
        item("heading-$section", "section-heading") { Text(if (allDay) "All day" else "Scheduled", style = MaterialTheme.typography.titleSmall) }
        if (entries.isEmpty()) item("empty-$section", "empty") { Text(if (allDay) "No all-day items" else "Nothing scheduled") }
        items(entries, key = { "$section-${it.item.agenda?.occurrenceId ?: it.item.id}-${it.item.agenda?.memberId}" }, contentType = { "calendar-entry" }) { entry ->
            PorchlightCard(Modifier.fillMaxWidth()) { CalendarEntryContent(entry, zone, openItem, allDay) }
        }
    }
}

@Composable private fun CalendarDayContent(day: CalendarDay, zone: TimeZone, openItem: (AppRoute.Item) -> Unit,
    modifier: Modifier, includeYear: Boolean) {
    PorchlightCard(modifier) {
        Text(calendarDayHeading(day.date, includeYear), style = MaterialTheme.typography.titleMedium)
        if (day.hours != 24L) Text("${day.hours}-hour day · daylight saving transition")
        Text("All day", style = MaterialTheme.typography.titleSmall)
        if (day.allDay.isEmpty()) Text("No all-day items")
        day.allDay.forEach { CalendarEntryContent(it, zone, openItem, true) }
        Text("Scheduled", style = MaterialTheme.typography.titleSmall)
        if (day.timed.isEmpty()) Text("Nothing scheduled")
        day.timed.forEach { CalendarEntryContent(it, zone, openItem, false) }
    }
}
@Composable private fun CalendarEntryContent(entry: CalendarEntry, zone: TimeZone, openItem: (AppRoute.Item) -> Unit, allDay: Boolean) {
    HorizontalDivider()
    Text(entry.item.title, style = MaterialTheme.typography.titleSmall)
    calendarEntryLabels(entry, zone, allDay).forEach { Text(it) }
    if ("view" in entry.item.capabilities) ShellButton("Open ${entry.item.title}", "Open ${entry.item.title}", { openItem(calendarItemRoute(entry)) })
}
