package app.porchlight.shared.screens.board

import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.data.BoardDrop
import app.porchlight.shared.screens.today.belongsTo

val boardStatuses = listOf("backlog", "todo", "doing", "done")
fun statusLabel(status: String) = when (status) { "backlog" -> "Backlog"; "todo" -> "To Do"; "doing" -> "Doing"; "done" -> "Done"; else -> status }
data class BoardColumn(val status: String, val cards: List<ItemRead>)
/** Only the server projection is accepted: it owns next-open-per-series and seven-day Done retention. */
fun boardColumns(projection: List<ItemRead>, filter: String?, ownMember: String): List<BoardColumn> {
    val rows = projection.filter { filter.isNullOrEmpty() || filter == "household" || belongsTo(it, if (filter == "mine") ownMember else filter) }
    return boardStatuses.map { status -> BoardColumn(status, rows.filter { it.occurrence?.status == status }
        .sortedWith(compareBy({ it.occurrence!!.boardPosition }, { it.occurrence!!.id }))) }
}
fun relativeDrop(column: List<ItemRead>, row: ItemRead, delta: Int) = BoardDrop(row.occurrence!!.status,
    (column.indexOfFirst { it.occurrence?.id == row.occurrence.id } + delta).coerceIn(0, (column.size - 1).coerceAtLeast(0)))
