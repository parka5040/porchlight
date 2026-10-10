package app.porchlight.shared.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.*
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.*
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.theme.PorchlightTokens

/** Stable IDs also support household members with identical display names. */
data class ScopeOption(val id: String, val label: String, val description: String = label)

/** Equal, selection-stable segments. Share the viewport while whole words fit; otherwise flow
 * complete segments onto additional rows. Layout (not subcomposition) keeps intrinsic parents safe.
 * The check is always budgeted, but unselected labels have no empty leading slot.
 */
@Composable fun SegmentedChoice(options: List<ScopeOption>, selectedId: String?, onSelect: (String) -> Unit,
    modifier: Modifier = Modifier) {
    if (options.isEmpty()) return
    Layout(modifier = modifier.fillMaxWidth().selectableGroup(), content = {
        options.forEach { option -> key(option.id) {
            val selected = option.id == selectedId
            val p = LocalPorchlightPalette.current
            Button({ if (!selected) onSelect(option.id) }, Modifier.heightIn(min = PorchlightTokens.touchTarget.dp)
                .widthIn(min = PorchlightTokens.touchTarget.dp)
                .semantics { contentDescription = option.description; this.selected = selected; role = Role.RadioButton },
                shape = MaterialTheme.shapes.small,
                border = BorderStroke(PorchlightTokens.selectionStroke.dp, if (selected) p.primary else p.outline),
                colors = ButtonDefaults.buttonColors(containerColor = if (selected) p.selectedSurface else Color.Transparent,
                    contentColor = if (selected) p.primaryText else p.onSurface),
                contentPadding = PaddingValues(horizontal = PorchlightTokens.spaceInset.dp, vertical = PorchlightTokens.spaceSm.dp)) {
                ChoiceLabel(option.label, selected)
            }
        } }
    }, measurePolicy = remember { ChoiceMeasurePolicy() })
}

private class ChoiceMeasurePolicy : MeasurePolicy {
    private data class Grid(val columns: Int, val width: Int, val height: Int, val rows: Int)
    private fun Density.grid(items: List<IntrinsicMeasurable>, viewport: Int): Grid {
        val gap = PorchlightTokens.spaceXs.dp.roundToPx()
        val natural = items.maxOf { it.maxIntrinsicWidth(Constraints.Infinity) }
        val minimum = items.maxOf { it.minIntrinsicWidth(Constraints.Infinity) }
        val columns = (items.size downTo 1).firstOrNull { count ->
            viewport == Constraints.Infinity || (viewport - gap * (count - 1)) / count >= minimum
        } ?: 1
        val width = if (viewport == Constraints.Infinity) natural
            else minOf(natural, (viewport - gap * (columns - 1)) / columns).coerceAtLeast(0)
        return Grid(columns, width, items.maxOf { it.maxIntrinsicHeight(width) }, (items.size + columns - 1) / columns)
    }
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val grid = grid(measurables, constraints.maxWidth)
        val gap = PorchlightTokens.spaceXs.dp.roundToPx()
        val places = measurables.map { it.measure(Constraints.fixed(grid.width, grid.height)) }
        return layout(constraints.constrainWidth(grid.width * grid.columns + gap * (grid.columns - 1)),
            constraints.constrainHeight(grid.height * grid.rows + gap * (grid.rows - 1))) {
            places.forEachIndexed { index, place ->
                place.placeRelative(index % grid.columns * (grid.width + gap), index / grid.columns * (grid.height + gap))
            }
        }
    }
    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables.maxOf { it.minIntrinsicWidth(height) }
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables.maxOf { it.maxIntrinsicWidth(height) } * measurables.size + PorchlightTokens.spaceXs.dp.roundToPx() * (measurables.size - 1)
    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int {
        val grid = grid(measurables, width)
        return grid.height * grid.rows + PorchlightTokens.spaceXs.dp.roundToPx() * (grid.rows - 1)
    }
    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        minIntrinsicHeight(measurables, width)
}

/** Wrap at word boundaries normally; only an individually over-wide word gets a one-line
 * ellipsis. The button's full accessibility label and selection action are unaffected. */
@Composable private fun WholeWordChoiceText(label: String) {
    var ellipsize by remember { mutableStateOf(false) }
    Layout(content = {
        Text(label, modifier = if (ellipsize) Modifier.clearAndSetSemantics {} else Modifier, style = MaterialTheme.typography.labelLarge, textAlign = TextAlign.Center)
        Text(label, modifier = if (ellipsize) Modifier else Modifier.clearAndSetSemantics {}, style = MaterialTheme.typography.labelLarge, textAlign = TextAlign.Center,
            softWrap = false, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
    }, measurePolicy = remember { object : MeasurePolicy {
        fun choice(items: List<IntrinsicMeasurable>, width: Int) =
            if (items.first().minIntrinsicWidth(Constraints.Infinity) > width) 1 else 0
        override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
            val selected = choice(measurables, constraints.maxWidth)
            ellipsize = selected == 1
            val text = measurables[selected].measure(constraints)
            return layout(text.width, text.height) { text.placeRelative(0, 0) }
        }
        override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) = measurables.first().minIntrinsicWidth(height)
        override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) = measurables.first().maxIntrinsicWidth(height)
        override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = measurables[choice(measurables, width)].minIntrinsicHeight(width)
        override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) = measurables[choice(measurables, width)].maxIntrinsicHeight(width)
    } })
}

@Composable private fun ChoiceLabel(label: String, selected: Boolean) {
    val p = LocalPorchlightPalette.current
    Box {
        Layout(content = { WholeWordChoiceText(label) },
            modifier = Modifier.testTag("choice-group-$label"), measurePolicy = remember(selected) {
                object : MeasurePolicy {
                    fun Density.decoration() = (PorchlightTokens.iconSize + PorchlightTokens.spaceXs).dp.roundToPx()
                    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
                        val decoration = decoration()
                        val labelWidth = minOf(measurables.single().maxIntrinsicWidth(Constraints.Infinity),
                            (constraints.maxWidth - decoration).coerceAtLeast(0))
                        val text = measurables.single().measure(constraints.copy(minWidth = labelWidth, minHeight = 0, maxWidth = labelWidth))
                        val leading = if (selected) decoration else 0
                        val height = maxOf(text.height, PorchlightTokens.iconSize.dp.roundToPx())
                        return layout(text.width + leading, height) { text.placeRelative(leading, (height - text.height) / 2) }
                    }
                    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
                        measurables.single().maxIntrinsicWidth(height) + decoration()
                    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
                        measurables.single().minIntrinsicWidth(height) + decoration()
                    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
                        maxOf(measurables.single().maxIntrinsicHeight((width - decoration()).coerceAtLeast(0)), PorchlightTokens.iconSize.dp.roundToPx())
                    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
                        maxIntrinsicHeight(measurables, width)
                }
            })
        if (selected) Canvas(Modifier.matchParentSize().testTag("selection-$label")) {
            val icon = PorchlightTokens.iconSize.dp.toPx()
            val left = if (layoutDirection == LayoutDirection.Ltr) 0f else size.width - icon
            val top = (size.height - icon) / 2
            val stroke = PorchlightTokens.selectionStroke.dp.toPx()
            drawLine(p.primaryText, Offset(left + icon * .2f, top + icon * .5f), Offset(left + icon * .4f, top + icon * .7f), stroke)
            drawLine(p.primaryText, Offset(left + icon * .4f, top + icon * .7f), Offset(left + icon * .8f, top + icon * .3f), stroke)
        }
    }
}
