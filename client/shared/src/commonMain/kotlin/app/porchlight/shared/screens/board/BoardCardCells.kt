package app.porchlight.shared.screens.board

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.layout.*
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.ItemRead
import app.porchlight.shared.theme.PorchlightTokens

private data class BoardPart(val key: String, val status: String, val allocation: Boolean = false)
private class BoardGeometry {
    var cards = linkedMapOf<String, Pair<Int, Int>>()
    var scroll = 0
}

/** Eager, globally keyed cards: focus and semantics must include the complete board, not an
 * estimated viewport. Independent card slots retain focus even when a move changes their column.
 * All heights are measured, so restoration never depends on a discarded estimate cache.
 */
@Composable internal fun BoardCardCells(columns: List<BoardColumn>, modifier: Modifier,
    recordBounds: (String, Rect) -> Unit, card: @Composable (BoardColumn, ItemRead, Modifier) -> Unit) {
    if (columns.isEmpty()) return
    require(columns.map { it.status }.distinct().size == columns.size)
    val measurer = rememberTextMeasurer()
    val headingStyle = MaterialTheme.typography.titleLarge
    val bodyStyle = MaterialTheme.typography.bodyLarge
    val scroll = rememberScrollState()
    var anchorKey by rememberSaveable { mutableStateOf<String?>(null) }
    var anchorOffset by rememberSaveable { mutableIntStateOf(0) }
    val geometry = remember { BoardGeometry().also { it.scroll = scroll.value } }
    val keyedCards = remember(columns) { columns.flatMap { column -> column.cards.map { column to it } } }
    val traversal = remember(columns) { columns.flatMap { column ->
        listOf("heading-${column.status}") + column.cards.map { it.occurrence!!.id }
    }.mapIndexed { index, key -> key to index.toFloat() }.toMap() }
    fun anchor(cards: Map<String, Pair<Int, Int>>, position: Int) {
        val visible = cards.entries.filter { (_, rect) -> rect.first + rect.second > position }
            .minByOrNull { it.value.first }
        anchorKey = visible?.key
        anchorOffset = visible?.let { position - it.value.first } ?: 0
    }
    BoxWithConstraints(modifier.fillMaxSize()) {
        val viewport = constraints.maxHeight
        Layout(modifier = Modifier.fillMaxWidth().testTag("board-columns")
            .semantics { isTraversalGroup = true }.verticalScroll(scroll), content = {
            columns.forEach { column -> key("heading-${column.status}") {
                Column(Modifier.layoutId(BoardPart("heading-${column.status}", column.status))
                    .fillMaxWidth().testTag("board-content-${column.status}")) {
                    Text("${statusLabel(column.status)} (${column.cards.size})", Modifier.fillMaxWidth()
                        .testTag("board-heading-${column.status}")
                        .semantics { traversalIndex = traversal.getValue("heading-${column.status}") }, style = headingStyle)
                    if (column.cards.isEmpty()) Text("No cards")
                }
            } }
            keyedCards.forEach { (column, row) -> key(row.occurrence!!.id) {
                Column(Modifier.layoutId(BoardPart(row.occurrence!!.id, column.status)).fillMaxWidth()
                    .testTag("board-content-${column.status}")) {
                    card(column, row, Modifier.semantics {
                        isTraversalGroup = true
                        traversalIndex = traversal.getValue(row.occurrence!!.id)
                    })
                }
            } }
            columns.forEach { column -> key("allocation-${column.status}") {
                Box(Modifier.layoutId(BoardPart("allocation-${column.status}", column.status, true))
                    .testTag("board-cell-${column.status}")
                    .onGloballyPositioned { recordBounds(column.status, it.boundsInRoot()) })
            } }
        }) { measurables, constraints ->
            if (geometry.cards.isNotEmpty() && scroll.value != geometry.scroll) anchor(geometry.cards, scroll.value)
            val gap = PorchlightTokens.spaceMd.dp.roundToPx()
            val inset = PorchlightTokens.spaceInset.dp.roundToPx()
            val cap = PorchlightTokens.detailWidth.dp.roundToPx()
            val headings = columns.map { "${statusLabel(it.status)} (${it.cards.size})" }
            val minimum = maxOf(PorchlightTokens.railWidth.dp.roundToPx(), headings.maxOf {
                kotlin.math.ceil(measurer.measure(it, headingStyle).multiParagraph.intrinsics.minIntrinsicWidth).toInt()
            }).coerceAtMost(cap)
            val across = listOf(4, 2, 1).first { it == 1 || minimum * it + gap * (it - 1) <= constraints.maxWidth }
            val width = minOf(cap, (constraints.maxWidth - gap * (across - 1)) / across).coerceAtLeast(0)
            val parts = measurables.associateBy { it.layoutId as BoardPart }
            val measured = parts.filterKeys { !it.allocation }.mapValues { (_, measurable) -> measurable.measure(Constraints.fixedWidth(width)) }.toMutableMap()
            val minimumHeight = headings.maxOf { measurer.measure(it, headingStyle, constraints = Constraints(maxWidth = width)).size.height } +
                measurer.measure("No cards", bodyStyle).size.height + (PorchlightTokens.spaceInset + PorchlightTokens.touchTarget).dp.roundToPx()
            val positions = linkedMapOf<BoardPart, Pair<Int, Int>>()
            val cards = linkedMapOf<String, Pair<Int, Int>>()
            var top = 0
            columns.chunked(across).forEachIndexed { band, row ->
                var rowHeight = minimumHeight
                row.forEachIndexed { slot, column ->
                    val x = slot * (width + gap)
                    val heading = BoardPart("heading-${column.status}", column.status)
                    positions[heading] = x to top
                    var y = top + measured.getValue(heading).height
                    column.cards.forEach { item ->
                        y += inset
                        val part = BoardPart(item.occurrence!!.id, column.status)
                        positions[part] = x to y
                        val height = measured.getValue(part).height
                        cards[part.key] = y to height
                        y += height
                    }
                    rowHeight = maxOf(rowHeight, y - top)
                }
                if (band == (columns.size - 1) / across) rowHeight = maxOf(rowHeight, viewport - top)
                row.forEachIndexed { slot, column ->
                    val part = BoardPart("allocation-${column.status}", column.status, true)
                    // A drop region need not exceed the current viewport when the whole column is
                    // taller than Compose's encodable constraints. Card geometry remains complete.
                    val fitted = Constraints.fitPrioritizingWidth(minWidth = width, maxWidth = width, minHeight = rowHeight, maxHeight = rowHeight)
                    val virtual = fitted.maxHeight < rowHeight
                    val y = if (virtual) maxOf(top, scroll.value).coerceAtMost(top + rowHeight) else top
                    val height = if (virtual) minOf(viewport, top + rowHeight - y) else rowHeight
                    measured[part] = parts.getValue(part).measure(Constraints.fixed(width, height.coerceAtLeast(0)))
                    positions[part] = slot * (width + gap) to y
                }
                top += rowHeight + gap
            }
            val height = (top - gap).coerceAtLeast(0)
            anchorKey?.let { key -> cards[key]?.let { rect ->
                val desired = (rect.first + anchorOffset).coerceIn(0, (height - viewport).coerceAtLeast(0))
                scroll.dispatchRawDelta((desired - scroll.value).toFloat())
            } }
            geometry.cards = cards
            geometry.scroll = scroll.value
            anchor(cards, scroll.value)
            layout(constraints.maxWidth, height) {
                positions.entries.sortedBy { if (it.key.allocation) 0 else 1 }.forEach { (part, point) ->
                    measured.getValue(part).placeRelative(point.first, point.second)
                }
            }
        }
    }
}
