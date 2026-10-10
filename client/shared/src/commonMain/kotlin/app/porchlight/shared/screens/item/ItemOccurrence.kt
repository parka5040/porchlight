package app.porchlight.shared.screens.item

import app.porchlight.shared.api.models.OccurrenceRead
import app.porchlight.shared.data.CacheView

fun itemOccurrence(view: CacheView, context: String, occurrenceId: String?): OccurrenceRead? =
    view.occurrences.find { it.id == occurrenceId && it.itemId == context }
        ?: view.board.find { it.id == context }?.occurrence?.takeIf { occurrenceId == null || it.id == occurrenceId }
        ?: view.agenda.find { it.id == context && it.occurrence?.id == occurrenceId }?.occurrence?.takeIf { occurrenceId != null }
