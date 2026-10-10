package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class C3bModelTest {
    private val zone = TimeZone.of("America/New_York")
    @Test fun calendarWindowsRespectDstAndSundayWeekAndMonth() {
        val spring = calendarWindow(CalendarView.Day, LocalDate(2026, 3, 8), zone)
        assertEquals(23, (spring.to - spring.from).inWholeHours)
        val fall = calendarWindow(CalendarView.Day, LocalDate(2026, 11, 1), zone)
        assertEquals(25, (fall.to - fall.from).inWholeHours)
        assertEquals(LocalDate(2026, 10, 4), calendarWindow(CalendarView.Week, LocalDate(2026, 10, 7), zone).dates.first())
        assertEquals(35, calendarWindow(CalendarView.Month, LocalDate(2026, 10, 7), zone).dates.size)
        assertEquals(LocalDate(2026, 9, 27), calendarWindow(CalendarView.Month, LocalDate(2026, 10, 7), zone).dates.first())
        assertEquals(LocalDate(2026, 10, 8), adjacentDate(CalendarView.Day, LocalDate(2026, 10, 7), 1))
        assertEquals(LocalDate(2026, 11, 1), adjacentDate(CalendarView.Month, LocalDate(2026, 10, 31), 1))
    }
    @Test fun usesOnlyServerResolvedAgendaNotFloatingAnchorsAndSplitsMidnight() {
        val row = itemRead("floating", true).copy(timeMode = "floating", localStart = "2026-03-08T02:30:00",
            agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"self","occurrence_id":"o","item_id":"floating","starts_at":"2026-03-08T07:00:00Z","ends_at":"2026-03-09T05:00:00Z","derived_at":"2026-03-08T07:00:00Z"}"""))
        val window = calendarWindow(CalendarView.Week, LocalDate(2026, 3, 8), zone)
        val layout = calendarDays(window, listOf(row, row.copy(agenda = row.agenda!!.copy(memberId = "other"))), "self", zone)
        assertEquals("03:00", layout.first().timed.single().startLabel)
        assertEquals("00:00", layout[1].timed.single().startLabel)
        assertTrue(layout.first().timed.single().item.pendingDerivation)
        assertEquals(23, layout.first().hours)
        assertTrue(calendarDays(window, listOf(row.copy(agenda = null)), "self", zone).all { it.timed.isEmpty() })
    }
    @Test fun allDayDueDateAndHiddenConflictStayServerAuthoritative() {
        val row = itemRead("date").copy(agenda = WireJson.decodeFromString<AgendaRead>("""{"household_id":"h","member_id":"self","occurrence_id":"o","item_id":"date","due_date":"2026-10-07","derived_at":"2026-10-06T12:00:00Z"}"""),
            conflicts = listOf(WireJson.decodeFromString<ConflictRead>("""{"member_id":"self","occurrence_id":"o","other_item_id":null,"other_title":"Busy","starts_at":"2026-10-07T12:00:00Z","ends_at":"2026-10-07T13:00:00Z","kind":"hard"}""")))
        val day = calendarDays(calendarWindow(CalendarView.Day, LocalDate(2026, 10, 7), zone), listOf(row), "self", zone).single()
        assertEquals("Busy", day.allDay.single().item.conflicts!!.single().otherTitle)
        assertTrue(day.timed.isEmpty())
    }
    @Test fun todayDraftPreservesOrderScopePinnedIdAndRejectsInvalidDays() {
        val initial = settingsRead().settings
        val draft = TodayDraft(initial).toggle("overdue").move("overdue", -1).withDays("14")
        assertEquals(listOf("overdue", "today", "this_week", "needs_owner", "pinned_list"), draft.order)
        assertEquals(listOf("overdue", "today"), draft.value().todaySections.map { it.value })
        assertEquals(14, draft.value().thisWeekDays)
        assertEquals("household", draft.withScope("household").scope)
        assertNull(draft.withDays("0").validatedValue())
        assertNull(draft.withDays("abc").validatedValue())
        assertEquals(initial.pinnedListId, draft.value().pinnedListId)
    }
    @Test fun versionConflictRequiresExplicitReapplyAndAmbiguousRelockReusesKey() = runTest {
        val sent = mutableListOf<Triple<Int, TodaySettings, String>>()
        var replies = 0
        val controller = HouseholdSettingsController({ "key-${sent.size}" },
            put = { version, body, key ->
                sent += Triple(version, body, key)
                when (++replies) {
                    1 -> throw HouseholdLockedAfterReply(key)
                    2 -> WriteResult.VersionConflict(WireJson.encodeToJsonElement(settingsRead(4)), ApiProblem("version-conflict", 409, "Changed"))
                    else -> WriteResult.Success(settingsRead(5).copy(settings = body))
                }
            }, patch = { _, _, _, _ -> error("unused") })
        controller.saveToday(settingsRead(), TodayDraft(settingsRead().settings).withDays("9").value())
        assertEquals(SettingsWriteStatus.Ambiguous, controller.state.value.status)
        controller.retry()
        assertEquals(sent[0], sent[1])
        assertEquals(SettingsWriteStatus.Conflict, controller.state.value.status)
        assertEquals(2, sent.size)
        controller.reapply()
        assertEquals(4, sent[2].first)
        assertNotEquals(sent[1].third, sent[2].third)
        assertEquals(9, sent[2].second.thisWeekDays)
        assertEquals(SettingsWriteStatus.Saved, controller.state.value.status)
    }
    @Test fun validationMapsServerErrorsAndAllowsEditWithoutReplay() = runTest {
        val controller = HouseholdSettingsController({ "key" }, put = { _, _, _ ->
            throw ApiProblem.decode("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Invalid","errors":[{"field":"this_week_days","message":"must be 1..14"}]}""")
        }, patch = { _, _, _, _ -> error("unused") })
        controller.saveToday(settingsRead(), settingsRead().settings)
        assertEquals("must be 1..14", controller.state.value.fieldErrors["this_week_days"])
        controller.discard()
        assertFalse(controller.unresolved)
    }
    @Test fun pinnedPlaceholderCannotProduceAnInvalidServerDocument() {
        assertNull(TodayDraft(settingsRead().settings).toggle("pinned_list").validatedValue())
    }
    @Test fun lateFailureCannotChangeAnotherIdentityAndColorUsesVersionAndKey() = runTest {
        var epoch = 1L
        val member = WireJson.decodeFromString<MemberRead>("""{"id":"self","household_id":"h","display_name":"Sam Rivera","role":"user","color":"#FABD2F","initials":"SR","tzid":"UTC","tz_source":"manual","version":9,"pending_derivation":false}""")
        val sent = mutableListOf<String>()
        val controller = HouseholdSettingsController({ "color-key" }, put = { _, _, _ -> error("unused") }, patch = { id, version, body, key ->
            sent += "$id/$version/${body.color}/$key"
            epoch = 2L
            throw ApiProblem("validation-failed", 422, "Old identity")
        }, epoch = { epoch })
        controller.saveColor(member, "#83A598")
        assertEquals(listOf("self/9/#83A598/color-key"), sent)
        assertEquals(SettingsWriteStatus.Idle, controller.state.value.status)
        assertFalse(controller.unresolved)
    }
    @Test fun localResumeRejectsDeletedInvisibleAndUnknownListsWithoutNetwork() {
        val cache = CacheView(items = listOf(itemRead("visible"), itemRead("deleted").copy(deletedAt = Instant.parse("2026-10-06T12:00:00Z"))),
            board = listOf(itemRead("projection")))
        assertTrue(localRouteAvailable(AppRoute.Item("visible"), cache))
        assertTrue(localRouteAvailable(AppRoute.Item("projection"), cache))
        assertFalse(localRouteAvailable(AppRoute.Item("deleted"), cache))
        assertFalse(localRouteAvailable(AppRoute.Item("hidden"), cache))
        assertFalse(localRouteAvailable(AppRoute.Lists("gone"), cache))
        assertTrue(localRouteAvailable(AppRoute.Calendar(CalendarView.Week, "2026-10-07"), cache))
    }
}
fun settingsRead(version: Int = 1): SettingsRead = WireJson.decodeFromString("""{"member_id":"self","version":$version,"settings":{"today_sections":["today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}}""")
