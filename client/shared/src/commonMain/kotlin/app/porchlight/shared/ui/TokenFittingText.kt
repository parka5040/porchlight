package app.porchlight.shared.ui

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.*
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*

/** One semantic node for all typography variants. Measurement selects and places one child,
 * without feeding selection back into composition. The layout-result holder is not snapshot state:
 * the stable semantics action reads the most recently measured text, including on the resize frame.
 * textModifier belongs to the common wrapper, so layout modifiers affect every variant equally. */
@Composable internal fun TokenFittingText(text: String, styles: List<TextStyle>, color: Color,
    modifier: Modifier = Modifier, textModifier: Modifier = Modifier, textAlign: TextAlign = TextAlign.Start) {
    val result = remember { FittingTextResult() }
    Layout(modifier = modifier.then(textModifier).semantics {
        this.text = androidx.compose.ui.text.AnnotatedString(text)
        getTextLayoutResult { results -> result.layout?.let { results.add(it) } ?: false }
    }, content = {
        styles.forEach { style ->
            Text(text, Modifier.clearAndSetSemantics {}.fillMaxWidth(), color = color, style = style, textAlign = textAlign,
                maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis, onTextLayout = { result.layout = it })
        }
    }, measurePolicy = remember { FittingTextPolicy() })
}

private class FittingTextResult { var layout: androidx.compose.ui.text.TextLayoutResult? = null }

private class FittingTextPolicy : MeasurePolicy {
    private fun choice(items: List<IntrinsicMeasurable>, width: Int) =
        items.firstOrNull { it.maxIntrinsicWidth(Constraints.Infinity) <= width } ?: items.last()
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val selected = choice(measurables, constraints.maxWidth) as Measurable
        val place = selected.measure(constraints)
        return layout(place.width, place.height) { place.placeRelative(0, 0) }
    }
    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables.last().maxIntrinsicWidth(height)
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables.first().maxIntrinsicWidth(height)
    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        choice(measurables, width).minIntrinsicHeight(width)
    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        choice(measurables, width).maxIntrinsicHeight(width)
}
