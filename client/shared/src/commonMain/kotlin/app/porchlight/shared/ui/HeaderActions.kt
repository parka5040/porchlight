package app.porchlight.shared.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.theme.PorchlightTokens

/** Content-sized actions with the same outline, padding and touch floor as the collection controls. */
@Composable fun PrimaryAction(label: String, onClick: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) =
    HeaderAction(label, onClick, true, modifier, enabled)

@Composable fun SecondaryAction(label: String, onClick: () -> Unit, modifier: Modifier = Modifier,
    enabled: Boolean = true, description: String = label) = HeaderAction(label, onClick, false, modifier, enabled, description)

@Composable fun ActionListRow(label: String, onClick: () -> Unit, modifier: Modifier = Modifier) =
    HeaderAction(label, onClick, false, modifier.fillMaxWidth(), true, textAlign = TextAlign.Start)

@Composable private fun HeaderAction(label: String, onClick: () -> Unit, primary: Boolean, modifier: Modifier,
    enabled: Boolean, description: String = label, textAlign: TextAlign = TextAlign.Center) {
    val p = LocalPorchlightPalette.current
    Button(onClick, modifier.heightIn(min = PorchlightTokens.touchTarget.dp).widthIn(min = PorchlightTokens.touchTarget.dp)
        .semantics { contentDescription = description }, enabled = enabled,
        shape = MaterialTheme.shapes.small,
        border = if (primary) null else BorderStroke(PorchlightTokens.selectionStroke.dp, p.outline),
        colors = ButtonDefaults.buttonColors(containerColor = if (primary) p.primary else Color.Transparent,
            contentColor = if (primary) p.onPrimary else p.onSurface),
        contentPadding = PaddingValues(horizontal = PorchlightTokens.spaceMd.dp, vertical = PorchlightTokens.spaceSm.dp)) {
        Text(label, modifier = if (textAlign == TextAlign.Start) Modifier.fillMaxWidth() else Modifier,
            style = MaterialTheme.typography.labelLarge, textAlign = textAlign)
    }
}

/** Weights supplied by callers cap long labels; intrinsic height keeps paired surfaces aligned. */
@Composable fun ActionRow(modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
    Row(modifier.fillMaxWidth().height(IntrinsicSize.Min),
        horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp), content = content)
}

enum class ActionIcon { Previous, Next, Search, Settings }

/** Bundled Canvas line icons, like the shell navigation; no icon/font dependency. */
@Composable fun IconAction(description: String, icon: ActionIcon, onClick: () -> Unit, modifier: Modifier = Modifier,
    outlined: Boolean = true) {
    val p = LocalPorchlightPalette.current
    val target = modifier.heightIn(min = PorchlightTokens.touchTarget.dp).widthIn(min = PorchlightTokens.touchTarget.dp)
        .semantics { contentDescription = description }
    if (outlined) OutlinedButton(onClick, target, shape = MaterialTheme.shapes.small,
        border = BorderStroke(PorchlightTokens.selectionStroke.dp, p.outline),
        contentPadding = PaddingValues(PorchlightTokens.spaceInset.dp),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = p.onSurface)) { ActionArtwork(icon) }
    else IconButton(onClick, target) { ActionArtwork(icon) }
}

@Composable private fun ActionArtwork(icon: ActionIcon) {
    val p = LocalPorchlightPalette.current
    Canvas(Modifier.size(PorchlightTokens.iconSize.dp)) {
        val stroke = PorchlightTokens.selectionStroke.dp.toPx()
        if (icon == ActionIcon.Settings) {
            drawCircle(p.onSurface, size.minDimension * .3f, center, style = Stroke(stroke))
            drawCircle(p.onSurface, size.minDimension * .1f, center, style = Stroke(stroke))
            for (tooth in 0 until 8) {
                val angle = tooth * kotlin.math.PI / 4
                val direction = Offset(kotlin.math.cos(angle).toFloat(), kotlin.math.sin(angle).toFloat())
                drawLine(p.onSurface, center + direction * (size.minDimension * .28f),
                    center + direction * (size.minDimension * .45f), stroke)
            }
        } else if (icon == ActionIcon.Search) {
            drawCircle(p.onSurface, size.minDimension * .28f, Offset(size.width * .42f, size.height * .42f), style = Stroke(stroke))
            drawLine(p.onSurface, Offset(size.width * .63f, size.height * .63f), Offset(size.width * .86f, size.height * .86f), stroke)
        } else {
            val from = if (icon == ActionIcon.Previous) .65f else .35f
            val to = 1f - from
            drawLine(p.onSurface, Offset(size.width * from, size.height * .2f), Offset(size.width * to, size.height * .5f), stroke)
            drawLine(p.onSurface, Offset(size.width * to, size.height * .5f), Offset(size.width * from, size.height * .8f), stroke)
        }
    }
}
