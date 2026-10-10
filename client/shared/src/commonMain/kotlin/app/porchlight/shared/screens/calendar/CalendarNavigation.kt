package app.porchlight.shared.screens.calendar

import app.porchlight.shared.navigation.AppRoute

/** Shared by agenda and desktop grid navigation, separate from layout geometry. */
fun calendarItemRoute(entry: CalendarEntry): AppRoute.Item =
    AppRoute.Item(entry.item.id, entry.item.occurrence?.id ?: entry.item.agenda?.occurrenceId)
