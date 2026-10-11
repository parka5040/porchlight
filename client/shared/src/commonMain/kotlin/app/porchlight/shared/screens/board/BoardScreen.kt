package app.porchlight.shared.screens.board

import androidx.compose.runtime.*
import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.geometry.*
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.key.*
import androidx.compose.ui.layout.*
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.Constraints
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.screens.*
import app.porchlight.shared.ui.*
import app.porchlight.shared.theme.PorchlightTokens
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@Composable fun BoardScreen(filter: String? = null, navigate: (AppRoute) -> Unit = {}) {
    val data = LocalScreenServices.current?.data
    if (data == null) { EmptyState("Board", "Sign in and sync to load household data."); return }
    val view by data.cache.view.collectAsState()
    val writer = screenWrites("board")
    val scope = applicationWriteScope()
    val member = view.settings?.memberId.orEmpty()
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(data, filter) {
        try { data.board.refresh(if (filter == "mine") "mine" else "household"); error = null }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { error = "Could not load the board; showing saved projection." }
    }
    Column(Modifier.fillMaxSize().padding(PorchlightTokens.spaceMd.dp),
        verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        DataStatus(data, error, { scope.refreshSafely(data) })
        CollectionHeader(
            options = listOf(ScopeOption("mine", "Mine"), ScopeOption("household", "Household")),
            selectedId = filter ?: "household",
            onScopeChange = { navigate(AppRoute.Board(it)) },
            onNewItem = { navigate(AppRoute.NewItem) },
            onRefresh = { scope.refreshSafely(data) },
            additionalScopes = {
                SegmentedChoice(view.members.map { ScopeOption(it.id, it.displayName, "Filter board for ${it.displayName}") },
                    filter ?: "household", { navigate(AppRoute.Board(it)) })
            },
        )
        DesignPending(if (LocalScreenServices.current?.platform?.kind == app.porchlight.shared.PlatformKind.Android) "Mobile Board adaptation" else "Desktop Board content layout")
        WriteStatus(writer)
        BoardContent(boardColumns(view.board, filter, member), view.members, { navigate(AppRoute.Item(it.id, it.occurrence?.id)) }, { row, drop ->
            row.occurrence?.let { occurrence ->
                val replay = BoardMoveReplay()
                scope.launch { writer.submit(activeKey = { replay.activeKey }) { key -> data.occurrences.move(occurrence.id, occurrence.version, drop, key,
                    visible = { projection -> boardColumns(projection, filter, member).flatMap { it.cards } }, replay = replay).feedback() } }
            }
        }, Modifier.weight(1f), take = { row -> scope.launch { writer.submit { data.items.take(row.id, row.version, it).feedback() } } })
    }
}

/** Long-press drag lands before/after a visible card or in its column; buttons, keys and custom actions are equivalent. */
@Composable fun BoardContent(columns: List<BoardColumn>, members: List<MemberRead>, open: (ItemRead) -> Unit,
    move: (ItemRead, BoardDrop) -> Unit, modifier: Modifier = Modifier, take: (ItemRead) -> Unit = {}) {
    val bounds = remember { mutableMapOf<String, Rect>() }
    val columnBounds = remember { mutableMapOf<String, Rect>() }
    // Keep only identity across pointer events; resolve the row from the current visible projection at drop.
    var dragged by remember { mutableStateOf<String?>(null) }
    var point by remember { mutableStateOf(Offset.Zero) }
    fun moveChanged(row: ItemRead, drop: BoardDrop) {
        val sameColumn = columns.find { it.status == row.occurrence?.status }
        val currentIndex = sameColumn?.cards?.indexOfFirst { it.occurrence?.id == row.occurrence?.id }
        if (drop.status != row.occurrence?.status || drop.index != currentIndex) move(row, drop)
    }
    val latestMove by rememberUpdatedState(::moveChanged)
    var dragMoved by remember { mutableStateOf(false) }
    val latestColumns by rememberUpdatedState(columns)
    val columnContent: @Composable (BoardColumn, Modifier) -> Unit = { column, columnModifier ->
            Column(columnModifier,
                verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceInset.dp)) {
                Text("${statusLabel(column.status)} (${column.cards.size})", modifier = Modifier.fillMaxWidth().testTag("board-heading-${column.status}"), style = MaterialTheme.typography.titleLarge)
                if (column.cards.isEmpty()) Text("No cards")
                column.cards.forEach { row ->
                    val canMove = "change_status" in row.capabilities
                    val occurrence = row.occurrence!!
                    val actions = buildList {
                        if (canMove) {
                            add(CustomAccessibilityAction("Move up") { moveChanged(row, relativeDrop(column.cards, row, -1)); true })
                            add(CustomAccessibilityAction("Move down") { moveChanged(row, relativeDrop(column.cards, row, 1)); true })
                            boardStatuses.filter { it != column.status }.forEach { status -> add(CustomAccessibilityAction("Move to ${statusLabel(status)}") { moveChanged(row, BoardDrop(status, 0)); true }) }
                        }
                    }
                    var origin by remember(occurrence.id) { mutableStateOf(Offset.Zero) }
                    PorchlightCard(Modifier.fillMaxWidth().alpha(if (dragged == occurrence.id) 0.65f else 1f).testTag("board-card-${occurrence.id}").onGloballyPositioned {
                        origin = it.positionInRoot()
                        // Insertion uses the entire card, including the scrolled-off part. Only the
                        // column's hit region is clipped; clipping a card shifts its midpoint.
                        bounds[occurrence.id] = Rect(origin, Size(it.size.width.toFloat(), it.size.height.toFloat()))
                    }
                        .semantics { customActions = actions; contentDescription = "Board card ${row.title}" }
                        .onKeyEvent { event ->
                            if (!canMove || event.type != KeyEventType.KeyDown || !event.isAltPressed) false else when (event.key) {
                                Key.DirectionUp -> { moveChanged(row, relativeDrop(column.cards, row, -1)); true }
                                Key.DirectionDown -> { moveChanged(row, relativeDrop(column.cards, row, 1)); true }
                                Key.DirectionLeft, Key.DirectionRight -> { val step = if (event.key == Key.DirectionLeft) -1 else 1
                                    val target = boardStatuses.getOrNull(boardStatuses.indexOf(column.status) + step)
                                    if (target != null) moveChanged(row, BoardDrop(target, 0)); target != null }
                                else -> false
                            }
                        }.focusable()
                        .pointerInput(occurrence.id, canMove, columns) {
                            if (canMove) detectDragGesturesAfterLongPress(
                                onDragStart = { dragged = occurrence.id; dragMoved = false; point = origin + it },
                                onDragCancel = { dragged = null },
                                onDragEnd = {
                                    val moving = latestColumns.flatMap { it.cards }.find { it.occurrence?.id == dragged && "change_status" in it.capabilities }
                                    val target = latestColumns.find { columnBounds[it.status]?.contains(point) == true }
                                    if (dragMoved && moving != null && target != null) {
                                        val without = target.cards.filter { it.occurrence?.id != moving.occurrence?.id }
                                        val index = without.indexOfFirst { bounds[it.occurrence!!.id]?.let { r -> point.y < r.center.y } == true }.let { if (it < 0) without.size else it }
                                        latestMove(moving, BoardDrop(target.status, index))
                                    }
                                    dragged = null
                                }, onDrag = { change, delta -> change.consume(); if (delta != Offset.Zero) dragMoved = true; point += delta })
                        }) {
                        ItemSummary(row, members)
                        ShellButton("Open ${row.title}", onClick = { open(row) })
                        if ("take" in row.capabilities) ShellButton("Take ${row.title}", onClick = { take(row) })
                        if (canMove) {
                            ShellButton("Move ${row.title} up", onClick = { moveChanged(row, relativeDrop(column.cards, row, -1)) })
                            ShellButton("Move ${row.title} down", onClick = { moveChanged(row, relativeDrop(column.cards, row, 1)) })
                            boardStatuses.filter { it != column.status }.forEach { status ->
                                ShellButton("Move ${row.title} to ${statusLabel(status)}", onClick = { moveChanged(row, BoardDrop(status, 0)) })
                            }
                        }
                    }
                }
            }
    }
    BoardCells(columns, modifier, { status, rect -> columnBounds[status] = rect }, columnContent)
}

private data class BoardCellId(val status: String, val allocation: Boolean)

/** Measures card content independently of its equal-height drop allocation.
 * Each slot is a column: zero roots still get a drop target, several roots stack vertically.
 */
@Composable internal fun BoardCells(columns: List<BoardColumn>, modifier: Modifier = Modifier,
    recordBounds: (String, Rect) -> Unit = { _, _ -> },
    columnContent: @Composable (BoardColumn, Modifier) -> Unit) {
    if (columns.isEmpty()) return
    require(columns.map { it.status }.distinct().size == columns.size) { "Board columns must have unique status identities" }
    // Pane width, not device kind. Only counted headings and tokens choose the arrangement:
    // a long user title (even one unbreakable token) must wrap within its own cell.
    val measurer = androidx.compose.ui.text.rememberTextMeasurer()
    val headingStyle = MaterialTheme.typography.titleLarge
    val bodyStyle = MaterialTheme.typography.bodyLarge
    BoxWithConstraints(modifier.fillMaxSize()) {
        val viewportHeight = constraints.maxHeight
        Layout(modifier = Modifier.fillMaxWidth().testTag("board-columns").verticalScroll(rememberScrollState()), content = {
            // Contents and drop allocations are siblings so the contents can be measured once with
            // loose height BEFORE sizing the equal-height row. Allocations contain no card content,
            // never clip it, and still supply the whole (including empty lower) cell hit region.
            columns.forEach { column -> key("content", column.status) {
                Column(Modifier.layoutId(BoardCellId(column.status, allocation = false)).fillMaxWidth()
                    .testTag("board-content-${column.status}")) {
                    columnContent(column, Modifier.fillMaxWidth())
                }
            } }
            columns.forEach { column -> key("allocation", column.status) {
                Box(Modifier.layoutId(BoardCellId(column.status, allocation = true)).testTag("board-cell-${column.status}")
                    .onGloballyPositioned { recordBounds(column.status, it.boundsInRoot()) })
            } }
        }) { measurables, constraints ->
            val gap = PorchlightTokens.spaceMd.dp.roundToPx()
            val cap = PorchlightTokens.detailWidth.dp.roundToPx()
            val headers = columns.map { "${statusLabel(it.status)} (${it.cards.size})" }
            val headerMinimum = headers.maxOf {
                kotlin.math.ceil(measurer.measure(it, headingStyle).multiParagraph.intrinsics.minIntrinsicWidth).toInt()
            }
            val minimum = maxOf(PorchlightTokens.railWidth.dp.roundToPx(), headerMinimum).coerceAtMost(cap)
            val across = listOf(4, 2, 1).first { it == 1 || minimum * it + gap * (it - 1) <= constraints.maxWidth }
            val width = minOf(cap, (constraints.maxWidth - gap * (across - 1)) / across).coerceAtLeast(0)
            val minimumHeight = headers.maxOf { measurer.measure(it, headingStyle,
                constraints = Constraints(maxWidth = width)).size.height } + measurer.measure("No cards", bodyStyle).size.height +
                (PorchlightTokens.spaceInset + PorchlightTokens.touchTarget).dp.roundToPx()
            // Real measurement, not intrinsic estimates: wrapped actions and chips must be free to
            // report their full height. Each measurable is measured only once (no subcomposition).
            val cells = measurables.associateBy { it.layoutId }
            check(cells.size == columns.size * 2) { "Each column must have one content wrapper and one drop allocation" }
            val contents = columns.map { cells.getValue(BoardCellId(it.status, allocation = false)).measure(Constraints.fixedWidth(width)) }
            val rows = contents.chunked(across)
            var top = 0
            val rowHeights = rows.mapIndexed { index, row ->
                val remaining = if (index == rows.lastIndex) (viewportHeight - top).coerceAtLeast(0) else 0
                maxOf(minimumHeight, row.maxOf { it.height }, remaining).also { top += it + gap }
            }
            val allocations = columns.mapIndexed { index, column ->
                cells.getValue(BoardCellId(column.status, allocation = true)).measure(Constraints.fixed(width, rowHeights[index / across]))
            }
            layout(constraints.maxWidth, rowHeights.sum() + gap * (rows.size - 1)) {
                var y = 0
                rows.forEachIndexed { rowIndex, row ->
                    row.forEachIndexed { columnIndex, content ->
                        val x = columnIndex * (width + gap)
                        allocations[rowIndex * across + columnIndex].placeRelative(x, y)
                        content.placeRelative(x, y)
                    }
                    y += rowHeights[rowIndex] + gap
                }
            }
        }
    }
}
