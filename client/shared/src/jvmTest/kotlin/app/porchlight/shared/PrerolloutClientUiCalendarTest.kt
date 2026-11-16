package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlinx.datetime.*
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientUiCalendarTest {
    @Test fun unrelatedCachePublicationDoesNotRebuildGrid() = runDesktopComposeUiTest(width = 800, height = 800) {
        var cache by mutableStateOf(CacheView())
        var calls = 0
        val builder: (CalendarWindow, List<app.porchlight.shared.api.models.ItemRead>, Set<String>, TimeZone, CalendarGridScale) -> CalendarGrid = { w, r, m, z, s ->
            calls++; calendarGrid(w, r, m, z, s)
        }
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            CalendarContent(AppRoute.Calendar(CalendarView.Agenda, "2026-10-07"), LocalDate(2026, 10, 7), TimeZone.UTC,
                null, cache.agenda, cache.members, true, false, cache.visibilityRevision.takeIf { it > 0 }?.toString(), {}, {}, {}, gridBuilder = builder)
        } }
        waitForIdle()
        assertEquals(1, calls)
        runOnIdle { cache = cache.copy(visibilityRevision = 1) }
        waitForIdle()
        assertEquals(1, calls, "Unchanged agenda/window must reuse the grid")
    }
    private fun row(id: String, start: String, end: String? = null, member: String = "self") = app.porchlight.shared.data.itemRead(id).copy(title = "Entry $id",
        agenda = app.porchlight.shared.transport.WireJson.decodeFromString<app.porchlight.shared.api.models.AgendaRead>("""{"household_id":"household","member_id":"$member","occurrence_id":"$id","item_id":"$id","starts_at":"$start","ends_at":${end?.let { "\"$it\"" } ?: "null"},"derived_at":"2026-10-07T12:00:00Z"}"""))
    @Test fun gridFiltersTheVisibleHalfOpenWindowBeforeOverlayWork() {
        val window = calendarWindow(CalendarView.Day, LocalDate(2026, 10, 7), TimeZone.UTC)
        val rows = listOf(row("past", "2026-10-01T12:00:00Z"), row("inside", "2026-10-07T12:00:00Z"),
            row("ends-at-start", "2026-10-06T12:00:00Z", "2026-10-07T00:00:00Z"),
            row("span", "2026-10-06T12:00:00Z", "2026-10-08T12:00:00Z"),
            row("at-end", "2026-10-08T00:00:00Z"), row("other", "2026-10-07T12:00:00Z", member = "other"))
        assertEquals(listOf("inside", "span"), calendarRowsInWindow(window, rows, setOf("self")).map { it.id })
    }
    @Test fun twoThousandEntriesInOneMobileDayAreIndependentlyLazy() = runDesktopComposeUiTest(width = 320, height = 1000) {
        val rows = List(2_000) { row(it.toString(), "2026-10-07T12:00:00Z") }
        setContent { PorchlightTheme(ThemeMode.Daylight) {
            CalendarContent(AppRoute.Calendar(CalendarView.Agenda, "2026-10-07"), LocalDate(2026, 10, 7), TimeZone.UTC,
                "self", rows, emptyList(), true, false, null, {}, {}, {})
        } }
        val count = onAllNodes(hasText("Entry", substring = true), useUnmergedTree = true).fetchSemanticsNodes().size
        assertTrue(count in 1..40, "One day must not compose all entries: $count")
    }
}
